use api_types::{
    CreateIssueCommentRequest, IssueComment, ListIssueCommentsResponse, MutationResponse,
};
use axum::{
    Router,
    extract::{Json, Path, Query, State},
    response::Json as ResponseJson,
    routing::get,
};
use serde::Deserialize;
use utils::response::ApiResponse;
use uuid::Uuid;

use crate::{DeploymentImpl, error::ApiError};

#[derive(Debug, Deserialize)]
pub(super) struct ListIssueCommentsQuery {
    pub issue_id: Uuid,
}

pub(super) fn router() -> Router<DeploymentImpl> {
    Router::new()
        .route(
            "/issue-comments",
            get(list_issue_comments).post(create_issue_comment),
        )
        .route(
            "/issue-comments/{issue_comment_id}",
            get(get_issue_comment).delete(delete_issue_comment),
        )
}

async fn list_issue_comments(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<ListIssueCommentsQuery>,
) -> Result<ResponseJson<ApiResponse<ListIssueCommentsResponse>>, ApiError> {
    let client = deployment.remote_client()?;
    let response = client.list_issue_comments(query.issue_id).await?;
    Ok(ResponseJson(ApiResponse::success(response)))
}

async fn get_issue_comment(
    State(deployment): State<DeploymentImpl>,
    Path(issue_comment_id): Path<Uuid>,
) -> Result<ResponseJson<ApiResponse<IssueComment>>, ApiError> {
    let client = deployment.remote_client()?;
    let response = client.get_issue_comment(issue_comment_id).await?;
    Ok(ResponseJson(ApiResponse::success(response)))
}

async fn create_issue_comment(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<CreateIssueCommentRequest>,
) -> Result<ResponseJson<ApiResponse<MutationResponse<IssueComment>>>, ApiError> {
    let client = deployment.remote_client()?;
    let response = client.create_issue_comment(&request).await?;
    Ok(ResponseJson(ApiResponse::success(response)))
}

async fn delete_issue_comment(
    State(deployment): State<DeploymentImpl>,
    Path(issue_comment_id): Path<Uuid>,
) -> Result<ResponseJson<ApiResponse<()>>, ApiError> {
    let client = deployment.remote_client()?;
    client.delete_issue_comment(issue_comment_id).await?;
    Ok(ResponseJson(ApiResponse::success(())))
}
