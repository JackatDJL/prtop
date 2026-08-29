use crate::{
    config::ProjectConfig,
    forge::{ForgeError, ForgeProvider, normalized_request},
    model::{ChangeRequest, ChangeRequestKind},
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
}
#[async_trait]
impl ForgeProvider for ForgejoProvider {
    fn name(&self) -> &str {
        &self.name
    }
    async fn list_change_requests(&self) -> Result<Vec<ChangeRequest>, ForgeError> {
        let token = self.token.as_ref().ok_or_else(|| {
            ForgeError::AuthenticationRequired(format!("{} (set PRTOP_FORGEJO_TOKEN)", self.host))
        })?;
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
    fn normalizes_forgejo_pull() {
        let row:Row=serde_json::from_str(r#"{"number":12,"title":"Renderer","user":{"login":"jack"},"head":{"ref":"new"},"base":{"ref":"main"},"updated_at":"2026-08-29T12:00:00Z"}"#).unwrap();
        assert_eq!(normalize("codeberg", "jack/foo", row).id.number, 12);
    }
}
