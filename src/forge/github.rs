use crate::{
    config::ProjectConfig,
    forge::{ForgeCapabilities, ForgeError, ForgeProvider, ReviewAction, auth, normalized_request},
    model::{ChangeRequest, ChangeRequestId, ChangeRequestKind, Comment, Person},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;

pub struct GitHubProvider {
    name: String,
    host: String,
    token: Option<String>,
    projects: Vec<String>,
}
impl GitHubProvider {
    pub fn new(name: String, host: String, projects: &[ProjectConfig]) -> Self {
        Self {
            projects: projects
                .iter()
                .filter(|p| p.forge == name)
                .map(|p| p.repo.clone())
                .collect(),
            name,
            host,
            token: std::env::var("PRTOP_GITHUB_TOKEN")
                .ok()
                .or_else(|| std::env::var("GITHUB_TOKEN").ok()),
        }
    }
    async fn credential(&self) -> Result<String, ForgeError> {
        match &self.token {
            Some(token) => Ok(token.clone()),
            None => {
                auth::resolve(
                    &["PRTOP_GITHUB_TOKEN", "GITHUB_TOKEN"],
                    "gh",
                    &["auth", "token"],
                    "run gh auth login or set PRTOP_GITHUB_TOKEN",
                )
                .await
            }
        }
    }
    fn api(&self, path: &str) -> String {
        if self.host == "github.com" {
            format!("https://api.github.com/{path}")
        } else {
            format!("https://{}/api/v3/{path}", self.host)
        }
    }
}
#[async_trait]
impl ForgeProvider for GitHubProvider {
    fn name(&self) -> &str {
        &self.name
    }
    fn capabilities(&self) -> ForgeCapabilities {
        ForgeCapabilities {
            comments: true,
            reviews: true,
            approve: true,
            request_changes: true,
            request_reviewers: true,
            edit_comments: true,
            delete_comments: true,
        }
    }
    async fn list_change_requests(&self) -> Result<Vec<ChangeRequest>, ForgeError> {
        let token = self.credential().await?;
        let client = reqwest::Client::new();
        let mut all = vec![];
        for repo in &self.projects {
            let url = if self.host == "github.com" {
                format!("https://api.github.com/repos/{repo}/pulls?state=open&per_page=100")
            } else {
                format!(
                    "https://{}/api/v3/repos/{repo}/pulls?state=open&per_page=100",
                    self.host
                )
            };
            let response = client
                .get(url)
                .header("Authorization", format!("Bearer {token}"))
                .header("Accept", "application/vnd.github+json")
                .header("User-Agent", "prtop")
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
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "prtop")
            .json(&serde_json::json!({"body": body}))
            .send()
            .await
            .map_err(network)?;
        ensure(response).await.map(|_| ())
    }
    async fn edit_comment(
        &self,
        _id: &ChangeRequestId,
        comment_id: &str,
        body: &str,
    ) -> Result<Comment, ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .patch(self.api(&format!("repos/issues/comments/{comment_id}")))
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "prtop")
            .json(&serde_json::json!({"body": body}))
            .send()
            .await
            .map_err(network)?;
        let row: IssueComment = ensure(response).await?.json().await.map_err(network)?;
        Ok(row.into_comment())
    }
    async fn delete_comment(
        &self,
        _id: &ChangeRequestId,
        comment_id: &str,
    ) -> Result<(), ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .delete(self.api(&format!("repos/issues/comments/{comment_id}")))
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "prtop")
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
            ReviewAction::Approve => "APPROVE",
            ReviewAction::RequestChanges => "REQUEST_CHANGES",
            ReviewAction::Comment => "COMMENT",
        };
        let response = reqwest::Client::new()
            .post(self.api(&format!(
                "repos/{}/pulls/{}/reviews",
                id.repository, id.number
            )))
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "prtop")
            .json(&serde_json::json!({"event":event,"body":body}))
            .send()
            .await
            .map_err(network)?;
        ensure(response).await.map(|_| ())
    }
    async fn search_reviewers(
        &self,
        _id: &ChangeRequestId,
        query: &str,
    ) -> Result<Vec<Person>, ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .get(self.api(&format!(
                "search/users?q={}",
                url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>()
            )))
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "prtop")
            .send()
            .await
            .map_err(network)?;
        let response = ensure(response).await?;
        let rows: SearchUsers = response.json().await.map_err(network)?;
        Ok(rows
            .items
            .into_iter()
            .map(|user| Person {
                login: user.login,
                name: None,
            })
            .collect())
    }
    async fn request_reviewer(
        &self,
        id: &ChangeRequestId,
        reviewer: &str,
    ) -> Result<(), ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .post(self.api(&format!(
                "repos/{}/pulls/{}/requested_reviewers",
                id.repository, id.number
            )))
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "prtop")
            .json(&serde_json::json!({"reviewers":[reviewer]}))
            .send()
            .await
            .map_err(network)?;
        ensure(response).await.map(|_| ())
    }
    async fn remove_reviewer(
        &self,
        id: &ChangeRequestId,
        reviewer: &str,
    ) -> Result<(), ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .delete(self.api(&format!(
                "repos/{}/pulls/{}/requested_reviewers",
                id.repository, id.number
            )))
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "prtop")
            .json(&serde_json::json!({"reviewers":[reviewer]}))
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
            "GitHub authentication expired".into(),
        )),
        403 => Err(ForgeError::PermissionDenied),
        404 => Err(ForgeError::NotFound),
        409 => Err(ForgeError::Conflict),
        422 => Err(ForgeError::Validation("GitHub rejected the request".into())),
        429 => Err(ForgeError::RateLimited {
            retry_after_seconds: None,
        }),
        _ => Err(ForgeError::Unavailable("GitHub request failed".into())),
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
    updated_at: DateTime<Utc>,
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
            updated_at: Some(self.updated_at),
            can_edit: true,
            can_delete: true,
            url: self.html_url,
            resolved: None,
        }
    }
}
#[derive(Deserialize)]
struct SearchUsers {
    items: Vec<User>,
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
    fn normalizes_github_pull() {
        let row: Row = serde_json::from_str(r#"{"number":4,"title":"Reader","user":{"login":"jack"},"head":{"ref":"fix"},"base":{"ref":"main"},"draft":false,"updated_at":"2026-08-29T12:00:00Z"}"#).unwrap();
        let item = normalize("github", "jack/prtop", row);
        assert_eq!(item.id.display(item.kind), "#4");
        assert_eq!(item.source_branch, "fix");
    }
    #[test]
    fn normalizes_comment_write_response() {
        let row: IssueComment = serde_json::from_str(r#"{"id":7,"body":"done","user":{"login":"jack"},"created_at":"2026-08-29T12:00:00Z","updated_at":"2026-08-29T12:01:00Z","html_url":"https://example.test/comment/7"}"#).unwrap();
        assert_eq!(row.into_comment().id, "7");
    }
}
