use crate::{
    config::ProjectConfig,
    forge::{ForgeCapabilities, ForgeError, ForgeProvider, ReviewAction, auth, normalized_request},
    model::{ChangeRequest, ChangeRequestId, ChangeRequestKind, Comment, Person},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;

pub struct ForgejoProvider {
    name: String,
    host: String,
    token: Option<String>,
    projects: Vec<String>,
}
impl ForgejoProvider {
    pub fn new(name: String, host: String, projects: &[ProjectConfig]) -> Self {
        Self {
            projects: projects
                .iter()
                .filter(|p| p.forge == name)
                .map(|p| p.repo.clone())
                .collect(),
            name,
            host,
            token: std::env::var("PRTOP_FORGEJO_TOKEN")
                .ok()
                .or_else(|| std::env::var("FORGEJO_TOKEN").ok()),
        }
    }
    async fn credential(&self) -> Result<String, ForgeError> {
        match &self.token {
            Some(token) => Ok(token.clone()),
            None => {
                auth::resolve(
                    &["PRTOP_FORGEJO_TOKEN", "FORGEJO_TOKEN"],
                    "tea",
                    &["login", "list"],
                    "set PRTOP_FORGEJO_TOKEN or configure tea",
                )
                .await
            }
        }
    }
    fn api(&self, path: &str) -> String {
        format!("https://{}/api/v1/{path}", self.host)
    }
}
#[async_trait]
impl ForgeProvider for ForgejoProvider {
    fn name(&self) -> &str {
        &self.name
    }
    fn capabilities(&self) -> ForgeCapabilities {
        ForgeCapabilities {
            comments: true,
            reviews: true,
            approve: true,
            request_changes: true,
            request_reviewers: false,
            edit_comments: true,
            delete_comments: true,
        }
    }
    async fn list_change_requests(&self) -> Result<Vec<ChangeRequest>, ForgeError> {
        let token = self.credential().await?;
        let client = reqwest::Client::new();
        let mut all = vec![];
        for repo in &self.projects {
            let url = format!(
                "https://{}/api/v1/repos/{repo}/pulls?state=open&limit=100",
                self.host
            );
            let response = client
                .get(url)
                .header("Authorization", format!("token {token}"))
                .send()
                .await
                .map_err(|e| ForgeError::Unavailable(e.to_string()))?
                .error_for_status()
                .map_err(|e| ForgeError::Unavailable(e.to_string()))?;
            let rows: Vec<Row> = response
                .json()
                .await
                .map_err(|e| ForgeError::Unavailable(e.to_string()))?;
            all.extend(rows.into_iter().map(|row| normalize(&self.name, repo, row)));
        }
        Ok(all)
    }
    async fn create_comment(&self, id: &ChangeRequestId, body: &str) -> Result<(), ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .post(self.api(&format!(
                "repos/{}/issues/{}/comments",
                id.repository, id.number
            )))
            .header("Authorization", format!("token {token}"))
            .json(&serde_json::json!({"body":body}))
            .send()
            .await
            .map_err(network)?;
        ensure(response).await.map(|_| ())
    }
    async fn edit_comment(
        &self,
        _: &ChangeRequestId,
        comment_id: &str,
        body: &str,
    ) -> Result<Comment, ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .patch(self.api(&format!("repos/issues/comments/{comment_id}")))
            .header("Authorization", format!("token {token}"))
            .json(&serde_json::json!({"body":body}))
            .send()
            .await
            .map_err(network)?;
        let row: IssueComment = ensure(response).await?.json().await.map_err(network)?;
        Ok(row.into_comment())
    }
    async fn delete_comment(
        &self,
        _: &ChangeRequestId,
        comment_id: &str,
    ) -> Result<(), ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .delete(self.api(&format!("repos/issues/comments/{comment_id}")))
            .header("Authorization", format!("token {token}"))
            .send()
            .await
            .map_err(network)?;
        ensure(response).await.map(|_| ())
    }
    async fn submit_review_action(
        &self,
        id: &ChangeRequestId,
        action: ReviewAction,
        body: &str,
    ) -> Result<(), ForgeError> {
        let token = self.credential().await?;
        let event = match action {
            ReviewAction::Approve => "APPROVED",
            ReviewAction::RequestChanges => "REQUEST_CHANGES",
            ReviewAction::Comment => "COMMENT",
        };
        let response = reqwest::Client::new()
            .post(self.api(&format!(
                "repos/{}/pulls/{}/reviews",
                id.repository, id.number
            )))
            .header("Authorization", format!("token {token}"))
            .json(&serde_json::json!({"event":event,"body":body}))
            .send()
            .await
            .map_err(network)?;
        ensure(response).await.map(|_| ())
    }
}
fn network(error: reqwest::Error) -> ForgeError {
    ForgeError::Unavailable(error.to_string())
}
async fn ensure(response: reqwest::Response) -> Result<reqwest::Response, ForgeError> {
    match response.status().as_u16() {
        200..=299 => Ok(response),
        401 => Err(ForgeError::AuthenticationRequired(
            "Forgejo authentication expired".into(),
        )),
        403 => Err(ForgeError::PermissionDenied),
        404 => Err(ForgeError::NotFound),
        409 => Err(ForgeError::Conflict),
        429 => Err(ForgeError::RateLimited {
            retry_after_seconds: None,
        }),
        _ => Err(ForgeError::Unavailable("Forgejo request failed".into())),
    }
}
#[derive(Deserialize)]
struct Row {
    number: u64,
    title: String,
    user: User,
    head: Branch,
    base: Branch,
    #[serde(default)]
    draft: bool,
    updated_at: DateTime<Utc>,
}
#[derive(Deserialize)]
struct User {
    login: String,
}
#[derive(Deserialize)]
struct Branch {
    #[serde(rename = "ref")]
    branch: String,
}
#[derive(Deserialize)]
struct IssueComment {
    id: u64,
    body: String,
    user: User,
    created_at: DateTime<Utc>,
    updated_at: Option<DateTime<Utc>>,
    html_url: Option<String>,
}
impl IssueComment {
    fn into_comment(self) -> Comment {
        Comment {
            id: self.id.to_string(),
            author: Person {
                login: self.user.login,
                name: None,
            },
            body: self.body,
            created_at: self.created_at,
            updated_at: self.updated_at,
            can_edit: true,
            can_delete: true,
            url: self.html_url,
            resolved: None,
        }
    }
}
fn normalize(forge: &str, repo: &str, row: Row) -> ChangeRequest {
    normalized_request(
        forge.into(),
        repo.into(),
        row.number,
        ChangeRequestKind::PullRequest,
        row.title,
        row.user.login,
        row.head.branch,
        row.base.branch,
        row.draft,
        row.updated_at,
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes_forgejo_pull() {
        let row:Row=serde_json::from_str(r#"{"number":12,"title":"Renderer","user":{"login":"jack"},"head":{"ref":"new"},"base":{"ref":"main"},"updated_at":"2026-08-29T12:00:00Z"}"#).unwrap();
        assert_eq!(normalize("codeberg", "jack/foo", row).id.number, 12);
    }
    #[test]
    fn normalizes_comment_write_response() {
        let row: IssueComment = serde_json::from_str(r#"{"id":7,"body":"done","user":{"login":"jack"},"created_at":"2026-08-29T12:00:00Z","updated_at":null,"html_url":null}"#).unwrap();
        assert_eq!(row.into_comment().id, "7");
    }
}
