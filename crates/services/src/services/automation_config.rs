use serde::Deserialize;
use std::path::PathBuf;
use uuid::Uuid;

/// Repository input for workspace creation.
#[derive(Debug, Clone, Deserialize)]
pub struct RepoConfig {
    pub repo_id: Uuid,
    pub branch: String,
}

/// What event triggers this rule.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AutomationTrigger {
    /// Fires whenever an issue is moved to the specified status.
    IssueStatusChanged { to: String },
}

/// Guard conditions that must ALL pass before the action executes.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AutomationCondition {
    /// Passes when no non-archived local workspace carries the issue's `simple_id` as its name.
    NoExistingWorkspace,
    /// Passes when the total number of remote issues in the given statuses is below `max`.
    WipLimit { max: usize, statuses: Vec<String> },
}

/// What the rule does when all conditions pass.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AutomationAction {
    /// Create a workspace and start the specified agent.
    CreateWorkspace {
        /// Executor name, e.g. "CLAUDE_CODE".
        executor: String,
        /// Optional agent mode, e.g. "ziwei".
        agent_id: Option<String>,
        /// Remote project UUID.
        project_id: Uuid,
        /// Repos to include in the workspace.
        repos: Vec<RepoConfig>,
    },
}

/// A single automation rule.
#[derive(Debug, Clone, Deserialize)]
pub struct AutomationRule {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub trigger: AutomationTrigger,
    #[serde(default)]
    pub conditions: Vec<AutomationCondition>,
    pub action: AutomationAction,
}

/// The full set of automation rules.
/// Loaded from `~/.config/vibe-kanban/automation-rules.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct AutomationConfig {
    pub rules: Vec<AutomationRule>,
}

fn default_true() -> bool {
    true
}

impl AutomationConfig {
    fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("vibe-kanban").join("automation-rules.toml"))
    }

    /// Load the automation config.
    /// Returns `None` if the file does not exist or cannot be parsed.
    pub fn load() -> Option<Self> {
        let path = Self::config_path()?;
        if !path.exists() {
            return None;
        }
        let contents = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to read automation-rules.toml: {e}");
                return None;
            }
        };
        match toml::from_str::<Self>(&contents) {
            Ok(config) => {
                tracing::info!(
                    "Loaded automation config: {} rule(s)",
                    config.rules.len()
                );
                Some(config)
            }
            Err(e) => {
                tracing::warn!("Failed to parse automation-rules.toml: {e}");
                None
            }
        }
    }
}
