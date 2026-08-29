use crate::{
    config::ProjectConfig,
    forge::{ForgeError, ForgeProvider, normalized_request},
    model::{ChangeRequest, ChangeRequestKind},
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
}
#[async_trait]
impl ForgeProvider for GitHubProvider {
    fn name(&self) -> &str {
        &self.name
    }
    async fn list_change_requests(&self) -> Result<Vec<ChangeRequest>, ForgeError> {
        let token = self.token.as_ref().ok_or_else(|| {
            ForgeError::AuthenticationRequired(format!(
                "{} (set PRTOP_GITHUB_TOKEN or run gh auth login)",
                self.host
            ))
        })?;
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
}
