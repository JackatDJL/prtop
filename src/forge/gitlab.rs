use crate::{
    config::ProjectConfig,
    forge::{ForgeError, ForgeProvider, normalized_request},
    model::{ChangeRequest, ChangeRequestKind},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;

pub struct GitLabProvider {
    name: String,
    host: String,
    token: Option<String>,
    projects: Vec<String>,
}
impl GitLabProvider {
    pub fn new(name: String, host: String, projects: &[ProjectConfig]) -> Self {
        Self {
            projects: projects
                .iter()
                .filter(|p| p.forge == name)
                .map(|p| p.repo.clone())
                .collect(),
            name,
            host,
            token: std::env::var("PRTOP_GITLAB_TOKEN")
                .ok()
                .or_else(|| std::env::var("GITLAB_TOKEN").ok()),
        }
    }
}
#[async_trait]
impl ForgeProvider for GitLabProvider {
    fn name(&self) -> &str {
        &self.name
    }
    async fn list_change_requests(&self) -> Result<Vec<ChangeRequest>, ForgeError> {
        let token = self.token.as_ref().ok_or_else(|| {
            ForgeError::AuthenticationRequired(format!(
                "{} (set PRTOP_GITLAB_TOKEN or run glab auth login)",
                self.host
            ))
        })?;
        let client = reqwest::Client::new();
        let mut all = vec![];
        for repo in &self.projects {
            let encoded = url::form_urlencoded::byte_serialize(repo.as_bytes()).collect::<String>();
            let url = format!(
                "https://{}/api/v4/projects/{encoded}/merge_requests?state=opened&per_page=100",
                self.host
            );
            let response = client
                .get(url)
                .header("PRIVATE-TOKEN", token)
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
    iid: u64,
    title: String,
    author: User,
    source_branch: String,
    target_branch: String,
    #[serde(default)]
    draft: bool,
    updated_at: DateTime<Utc>,
}
#[derive(Deserialize)]
struct User {
    username: String,
}
fn normalize(forge: &str, repo: &str, row: Row) -> ChangeRequest {
    normalized_request(
        forge.into(),
        repo.into(),
        row.iid,
        ChangeRequestKind::MergeRequest,
        row.title,
        row.author.username,
        row.source_branch,
        row.target_branch,
        row.draft,
        row.updated_at,
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes_gitlab_mr() {
        let row: Row=serde_json::from_str(r#"{"iid":43,"title":"Blocks","author":{"username":"jack"},"source_branch":"public","target_branch":"main","draft":true,"updated_at":"2026-08-29T12:00:00Z"}"#).unwrap();
        let item = normalize("work", "volt/volt.link", row);
        assert_eq!(item.id.display(item.kind), "!43");
        assert!(item.draft);
    }
}
