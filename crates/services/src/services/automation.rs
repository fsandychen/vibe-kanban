use std::{collections::HashMap, str::FromStr, time::Duration};

use api_types::{Issue, SearchIssuesRequest};
use db::{
    DBService,
    models::{
        requests::{
            CreateAndStartWorkspaceRequest, CreateAndStartWorkspaceResponse, LinkedIssueInfo,
            WorkspaceRepoInput,
        },
        workspace::Workspace,
    },
};
use executors::{executors::BaseCodingAgent, profile::ExecutorConfig};
use serde::Deserialize;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::services::remote_client::RemoteClient;
use super::automation_config::{
    AutomationAction, AutomationCondition, AutomationConfig, AutomationRule, AutomationTrigger,
};

/// Background service that polls the remote Vibe Kanban API and fires automation rules.
///
/// Spawned from `LocalDeployment::new()` alongside `PrMonitorService`.
/// Disabled (noop) when no remote client is configured or no config file is present.
pub struct AutomationService {
    db: DBService,
    remote_client: RemoteClient,
    config: AutomationConfig,
    poll_interval: Duration,
    /// In-session idempotency: issue IDs for which workspace creation was triggered.
    processed_issues: std::collections::HashSet<Uuid>,
    /// Cache: project_id → (status_name → status_id).  Populated on first use.
    status_cache: HashMap<Uuid, HashMap<String, Uuid>>,
    http_client: reqwest::Client,
}

impl AutomationService {
    /// Spawn the automation service as a background task.
    ///
    /// Returns a noop handle when `remote_client` is `None` (remote features disabled).
    pub async fn spawn(
        db: DBService,
        remote_client: Option<RemoteClient>,
        config: AutomationConfig,
    ) -> tokio::task::JoinHandle<()> {
        let Some(rc) = remote_client else {
            info!("AutomationService: no remote client configured, service disabled");
            return tokio::spawn(async {});
        };

        let service = Self {
            db,
            remote_client: rc,
            config,
            poll_interval: Duration::from_secs(60),
            processed_issues: std::collections::HashSet::new(),
            status_cache: HashMap::new(),
            http_client: reqwest::Client::new(),
        };

        tokio::spawn(async move {
            service.start().await;
        })
    }

    async fn start(mut self) {
        info!(
            "AutomationService starting with {} rule(s), poll interval {:?}",
            self.config.rules.len(),
            self.poll_interval
        );

        // Brief startup delay so the HTTP server has time to bind and write the port file
        // before we attempt any local API calls.
        tokio::time::sleep(Duration::from_secs(10)).await;

        let mut interval = tokio::time::interval(self.poll_interval);
        loop {
            interval.tick().await;
            self.check_all_rules().await;
        }
    }

    async fn check_all_rules(&mut self) {
        let rules = self.config.rules.clone();
        for rule in rules {
            if !rule.enabled {
                continue;
            }
            let trigger = rule.trigger.clone();
            match trigger {
                AutomationTrigger::IssueStatusChanged { to } => {
                    self.handle_issue_status_trigger(&rule, &to).await;
                }
            }
        }
    }

    /// Resolve a status name to its UUID for the given project, using a persistent cache.
    async fn resolve_status_id(&mut self, project_id: Uuid, status_name: &str) -> Option<Uuid> {
        if !self.status_cache.contains_key(&project_id) {
            match self.remote_client.list_project_statuses(project_id).await {
                Ok(resp) => {
                    let map: HashMap<String, Uuid> = resp
                        .project_statuses
                        .into_iter()
                        .map(|s| (s.name.clone(), s.id))
                        .collect();
                    self.status_cache.insert(project_id, map);
                }
                Err(e) => {
                    warn!(
                        "AutomationService: failed to fetch project statuses for {project_id}: {e}"
                    );
                    return None;
                }
            }
        }
        self.status_cache.get(&project_id)?.get(status_name).copied()
    }

    async fn handle_issue_status_trigger(
        &mut self,
        rule: &AutomationRule,
        target_status: &str,
    ) {
        let project_id = match &rule.action {
            AutomationAction::CreateWorkspace { project_id, .. } => *project_id,
        };

        // Resolve status name → UUID.
        let Some(status_id) = self.resolve_status_id(project_id, target_status).await else {
            warn!(
                "AutomationService: rule '{}' skipped — unknown status '{}'",
                rule.name, target_status
            );
            return;
        };

        // Fetch all issues currently in this status.
        let issues = match self
            .remote_client
            .search_issues(&SearchIssuesRequest {
                project_id,
                status_id: Some(status_id),
                status_ids: None,
                priority: None,
                parent_issue_id: None,
                search: None,
                simple_id: None,
                assignee_user_id: None,
                tag_id: None,
                tag_ids: None,
                sort_field: None,
                sort_direction: None,
                limit: None,
                offset: None,
            })
            .await
        {
            Ok(resp) => resp.issues,
            Err(e) => {
                error!(
                    "AutomationService: failed to list issues for rule '{}': {e}",
                    rule.name
                );
                return;
            }
        };

        if issues.is_empty() {
            debug!(
                "AutomationService: rule '{}' — no issues in status '{}'",
                rule.name, target_status
            );
            return;
        }

        info!(
            "AutomationService: rule '{}' — {} issue(s) in status '{}'",
            rule.name,
            issues.len(),
            target_status
        );

        for issue in issues {
            // In-session idempotency check.
            if self.processed_issues.contains(&issue.id) {
                debug!(
                    "AutomationService: {} already processed this session, skipping",
                    issue.simple_id
                );
                continue;
            }

            let rule_clone = rule.clone();
            let issue_clone = issue.clone();
            if self.passes_conditions(&rule_clone, &issue_clone).await {
                self.execute_action(&rule_clone, &issue_clone).await;
            }
        }
    }

    /// Returns `true` if all conditions in the rule are satisfied for the given issue.
    async fn passes_conditions(&mut self, rule: &AutomationRule, issue: &Issue) -> bool {
        let project_id = match &rule.action {
            AutomationAction::CreateWorkspace { project_id, .. } => *project_id,
        };

        for condition in &rule.conditions {
            match condition {
                AutomationCondition::NoExistingWorkspace => {
                    match Workspace::fetch_all(&self.db.pool).await {
                        Ok(workspaces) => {
                            let exists = workspaces
                                .iter()
                                .any(|w| !w.archived && w.name.as_deref() == Some(&issue.simple_id));
                            if exists {
                                info!(
                                    "AutomationService: {} already has a workspace, skipping",
                                    issue.simple_id
                                );
                                return false;
                            }
                        }
                        Err(e) => {
                            warn!("AutomationService: failed to query local workspaces: {e}");
                            return false;
                        }
                    }
                }
                AutomationCondition::WipLimit { max, statuses } => {
                    let mut wip_count = 0usize;
                    let statuses = statuses.clone();
                    for status_name in &statuses {
                        let Some(sid) =
                            self.resolve_status_id(project_id, status_name).await
                        else {
                            warn!(
                                "AutomationService: unknown WIP status '{status_name}', skipping"
                            );
                            continue;
                        };
                        match self
                            .remote_client
                            .search_issues(&SearchIssuesRequest {
                                project_id,
                                status_id: Some(sid),
                                status_ids: None,
                                priority: None,
                                parent_issue_id: None,
                                search: None,
                                simple_id: None,
                                assignee_user_id: None,
                                tag_id: None,
                                tag_ids: None,
                                sort_field: None,
                                sort_direction: None,
                                limit: None,
                                offset: None,
                            })
                            .await
                        {
                            Ok(resp) => wip_count += resp.issues.len(),
                            Err(e) => {
                                warn!(
                                    "AutomationService: WIP count query failed for '{status_name}': {e}"
                                );
                            }
                        }
                    }
                    if wip_count >= *max {
                        info!(
                            "AutomationService: WIP limit reached ({wip_count}/{max}), skipping {}",
                            issue.simple_id
                        );
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Execute the rule's action for the given issue.
    async fn execute_action(&mut self, rule: &AutomationRule, issue: &Issue) {
        let AutomationAction::CreateWorkspace {
            executor,
            agent_id,
            project_id,
            repos,
        } = &rule.action;

        // Parse executor string → BaseCodingAgent.
        let base_executor = match BaseCodingAgent::from_str(executor) {
            Ok(e) => e,
            Err(_) => {
                error!(
                    "AutomationService: unknown executor '{}' in rule '{}'",
                    executor, rule.name
                );
                return;
            }
        };

        let workspace_repos: Vec<WorkspaceRepoInput> = repos
            .iter()
            .map(|r| WorkspaceRepoInput {
                repo_id: r.repo_id,
                target_branch: r.branch.clone(),
            })
            .collect();

        let prompt = format!("處理 Issue {}（{}）", issue.simple_id, issue.id);

        let request = CreateAndStartWorkspaceRequest {
            name: Some(issue.simple_id.clone()),
            repos: workspace_repos,
            linked_issue: Some(LinkedIssueInfo {
                remote_project_id: *project_id,
                issue_id: issue.id,
            }),
            executor_config: ExecutorConfig {
                executor: base_executor,
                variant: None,
                model_id: None,
                agent_id: agent_id.clone(),
                reasoning_id: None,
                permission_policy: None,
            },
            prompt,
            attachment_ids: None,
        };

        // Discover the local server port from the port file.
        let port = match utils::port_file::read_port_file("vibe-kanban").await {
            Ok(p) => p,
            Err(e) => {
                warn!(
                    "AutomationService: could not read VK port file: {e}. \
                     Is the server running?"
                );
                return;
            }
        };

        let start_url = format!("http://127.0.0.1:{port}/api/workspaces/start");
        info!(
            "AutomationService: creating workspace for {} → {}",
            issue.simple_id, start_url
        );

        // POST to create and start the workspace.
        let http_resp = match self
            .http_client
            .post(&start_url)
            .json(&request)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                error!(
                    "AutomationService: workspace creation request failed for {}: {e}",
                    issue.simple_id
                );
                return;
            }
        };

        if !http_resp.status().is_success() {
            let status = http_resp.status();
            let body = http_resp.text().await.unwrap_or_default();
            error!(
                "AutomationService: workspace creation returned HTTP {status} for {}: {body}",
                issue.simple_id
            );
            return;
        }

        // Parse the workspace ID from the response.
        #[derive(Deserialize)]
        struct ApiResponse {
            data: Option<CreateAndStartWorkspaceResponse>,
        }

        let workspace_id = match http_resp.json::<ApiResponse>().await {
            Ok(r) => match r.data {
                Some(d) => d.workspace.id,
                None => {
                    error!(
                        "AutomationService: workspace creation response missing data for {}",
                        issue.simple_id
                    );
                    return;
                }
            },
            Err(e) => {
                error!(
                    "AutomationService: failed to parse workspace creation response for {}: {e}",
                    issue.simple_id
                );
                return;
            }
        };

        info!(
            "AutomationService: workspace {} created for {} — linking to remote issue",
            workspace_id, issue.simple_id
        );

        // Link the workspace to the remote issue.
        let link_url = format!("http://127.0.0.1:{port}/api/workspaces/{workspace_id}/links");
        let link_body = serde_json::json!({
            "project_id": project_id,
            "issue_id": issue.id,
        });

        match self
            .http_client
            .post(&link_url)
            .json(&link_body)
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => {
                info!(
                    "AutomationService: workspace {} linked to issue {} ✓",
                    workspace_id, issue.simple_id
                );
            }
            Ok(r) => {
                // Linking failure is non-fatal — workspace and execution are already started.
                warn!(
                    "AutomationService: workspace link returned HTTP {} for {} (non-fatal)",
                    r.status(),
                    issue.simple_id
                );
            }
            Err(e) => {
                warn!(
                    "AutomationService: workspace link request failed for {} (non-fatal): {e}",
                    issue.simple_id
                );
            }
        }

        // Mark as processed to prevent re-triggering within this service session.
        self.processed_issues.insert(issue.id);
    }
}
