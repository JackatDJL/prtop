use crate::{
    config::ProjectConfig,
    forge::{
        ForgeCapabilities, ForgeError, ForgeProvider, MergeOutcome, MergeStrategy, Milestone,
        NewChangeRequest, RepositoryInfo, RequestPatch, ReviewAction, auth, normalized_request,
    },
    model::{
        ChangeRequest, ChangeRequestId, ChangeRequestKind, Comment, Label, Person,
        RequestState, ReviewState, Reviewer,
    },
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
    async fn get_pull(&self, token: &str, repository: &str, number: u64) -> Result<Row, ForgeError> {
        let response = reqwest::Client::new()
            .get(self.api(&format!("repos/{repository}/pulls/{number}")))
            .header("Authorization", format!("token {token}"))
            .send()
            .await
            .map_err(network)?;
        ensure(response).await?.json().await.map_err(network)
    }
    async fn fetch_full(
        &self,
        token: &str,
        id: &ChangeRequestId,
    ) -> Result<ChangeRequest, ForgeError> {
        let row = self.get_pull(token, &id.repository, id.number).await?;
        Ok(normalize(&self.name, &id.repository, row))
    }
    /// Forgejo issue edits carry labels (ids), assignees (logins) and the milestone (id).
    async fn patch_issue(
        &self,
        token: &str,
        id: &ChangeRequestId,
        body: serde_json::Value,
    ) -> Result<(), ForgeError> {
        let response = reqwest::Client::new()
            .patch(self.api(&format!(
                "repos/{}/issues/{}",
                id.repository, id.number
            )))
            .header("Authorization", format!("token {token}"))
            .json(&body)
            .send()
            .await
            .map_err(network)?;
        ensure(response).await.map(|_| ())
    }
    async fn label_ids(
        &self,
        token: &str,
        repository: &str,
        wanted: &[String],
    ) -> Result<Vec<u64>, ForgeError> {
        let response = reqwest::Client::new()
            .get(self.api(&format!("repos/{repository}/labels?limit=100")))
            .header("Authorization", format!("token {token}"))
            .send()
            .await
            .map_err(network)?;
        let rows: Vec<LabelRow> = ensure(response).await?.json().await.map_err(network)?;
        wanted
            .iter()
            .map(|name| rows.iter().find(|row| &row.name == name).map(|row| row.id))
            .collect::<Vec<Option<u64>>>()
            .try_into_result(wanted)
    }
    async fn milestone_id(
        &self,
        token: &str,
        repository: &str,
        title: &str,
    ) -> Result<u64, ForgeError> {
        let response = reqwest::Client::new()
            .get(self.api(&format!("repos/{repository}/milestones?state=open&limit=100")))
            .header("Authorization", format!("token {token}"))
            .send()
            .await
            .map_err(network)?;
        let rows: Vec<MilestoneRow> = ensure(response).await?.json().await.map_err(network)?;
        rows.iter()
            .find(|row| row.title == title)
            .map(|row| row.id)
            .ok_or_else(|| ForgeError::Validation(format!("milestone {title} not found")))
    }
}
trait IntoLabelIds {
    fn try_into_result(self, wanted: &[String]) -> Result<Vec<u64>, ForgeError>;
}
impl IntoLabelIds for Vec<Option<u64>> {
    fn try_into_result(self, wanted: &[String]) -> Result<Vec<u64>, ForgeError> {
        if self.iter().all(|id| id.is_some()) {
            Ok(self.into_iter().flatten().collect())
        } else {
            Err(ForgeError::Validation(
                format!("unknown label(s): {}", wanted.join(", ")),
            ))
        }
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
            // Forgejo Actions is optional and varies by server release. Discovery can later
            // promote these flags; the conservative default never offers unsafe CI writes.
            create_change_request: true,
            edit_title: true,
            edit_description: true,
            labels: true,
            assignees: true,
            milestone: true,
            // The official edit schema has no draft field; drafts are create-only here.
            draft_transition: false,
            close: true,
            reopen: true,
            merge: true,
            merge_commit: true,
            squash_merge: true,
            rebase_merge: true,
            ..ForgeCapabilities::default()
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
    async fn get_change_request(&self, id: &ChangeRequestId) -> Result<ChangeRequest, ForgeError> {
        let token = self.credential().await?;
        self.fetch_full(&token, id).await
    }
    async fn get_repository(&self, repository: &str) -> Result<RepositoryInfo, ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .get(self.api(&format!("repos/{repository}")))
            .header("Authorization", format!("token {token}"))
            .send()
            .await
            .map_err(network)?;
        let row: RepositoryRow = ensure(response).await?.json().await.map_err(network)?;
        Ok(RepositoryInfo {
            default_branch: row.default_branch,
        })
    }
    async fn create_change_request(
        &self,
        input: &NewChangeRequest,
        repository: &str,
    ) -> Result<ChangeRequest, ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .post(self.api(&format!("repos/{repository}/pulls")))
            .header("Authorization", format!("token {token}"))
            .json(&serde_json::json!({
                "title": input.title,
                "body": input.body,
                "head": input.source_branch,
                "base": input.target_branch,
                "draft": input.draft,
            }))
            .send()
            .await
            .map_err(network)?;
        let row: Row = ensure(response).await?.json().await.map_err(network)?;
        let mut created = normalize(&self.name, repository, row);
        // Best-effort metadata after creation; the targeted refresh reconciles provider truth.
        if !input.labels.is_empty() {
            if let Ok(ids) = self.label_ids(&token, repository, &input.labels).await {
                let _ = self
                    .patch_issue(
                        &token,
                        &created.id,
                        serde_json::json!({"labels": ids}),
                    )
                    .await;
            }
        }
        if !input.assignees.is_empty() {
            let _ = self
                .patch_issue(
                    &token,
                    &created.id,
                    serde_json::json!({"assignees": input.assignees}),
                )
                .await;
        }
        if let Some(milestone) = &input.milestone {
            if let Ok(id) = self.milestone_id(&token, repository, milestone).await {
                let _ = self
                    .patch_issue(&token, &created.id, serde_json::json!({"milestone": id}))
                    .await;
            }
        }
        Ok(created)
    }
    async fn update_change_request(
        &self,
        id: &ChangeRequestId,
        patch: &RequestPatch,
    ) -> Result<ChangeRequest, ForgeError> {
        let token = self.credential().await?;
        if let Some(draft) = patch.draft {
            return Err(ForgeError::Validation(format!(
                "Forgejo does not support {} drafts through the edit API",
                if draft { "marking" } else { "unmarking" }
            )));
        }
        if let Some(title) = &patch.title {
            self.patch_issue(&token, id, serde_json::json!({"title": title}))
                .await?;
        }
        if let Some(body) = &patch.body {
            self.patch_issue(&token, id, serde_json::json!({"body": body}))
                .await?;
        }
        if let Some(state) = patch.state {
            let state = match state {
                RequestState::Open => "open",
                RequestState::Closed | RequestState::Merged => "closed",
            };
            let response = reqwest::Client::new()
                .patch(self.api(&format!(
                    "repos/{}/pulls/{}",
                    id.repository, id.number
                )))
                .header("Authorization", format!("token {token}"))
                .json(&serde_json::json!({"state": state}))
                .send()
                .await
                .map_err(network)?;
            ensure(response).await?;
        }
        if let Some(labels) = &patch.labels {
            let ids = self.label_ids(&token, &id.repository, labels).await?;
            self.patch_issue(&token, id, serde_json::json!({"labels": ids}))
                .await?;
        }
        if let Some(assignees) = &patch.assignees {
            self.patch_issue(&token, id, serde_json::json!({"assignees": assignees}))
            .await?;
        }
        if let Some(milestone) = &patch.milestone {
            let milestone_id = match milestone {
                Some(title) => Some(self.milestone_id(&token, &id.repository, title).await?),
                None => Some(0),
            };
            if let Some(milestone_id) = milestone_id {
                self.patch_issue(&token, id, serde_json::json!({"milestone": milestone_id}))
                    .await?;
            }
        }
        self.fetch_full(&token, id).await
    }
    async fn list_labels(&self, repository: &str) -> Result<Vec<Label>, ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .get(self.api(&format!("repos/{repository}/labels?limit=100")))
            .header("Authorization", format!("token {token}"))
            .send()
            .await
            .map_err(network)?;
        let rows: Vec<LabelRow> = ensure(response).await?.json().await.map_err(network)?;
        Ok(rows
            .into_iter()
            .map(|row| Label {
                name: row.name,
                color: row.color,
            })
            .collect())
    }
    async fn set_labels(
        &self,
        id: &ChangeRequestId,
        names: &[String],
    ) -> Result<Vec<Label>, ForgeError> {
        let updated = self.update_change_request(
            id,
            &RequestPatch {
                labels: Some(names.to_vec()),
                ..RequestPatch::default()
            },
        )
        .await?;
        Ok(updated.labels)
    }
    async fn search_assignees(
        &self,
        repository: &str,
        query: &str,
    ) -> Result<Vec<Person>, ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .get(self.api(&format!(
                "repos/{repository}/assignees?limit=100"
            )))
            .header("Authorization", format!("token {token}"))
            .send()
            .await
            .map_err(network)?;
        let rows: Vec<User> = ensure(response).await?.json().await.map_err(network)?;
        let query = query.to_lowercase();
        Ok(rows
            .into_iter()
            .filter(|user| query.is_empty() || user.login.to_lowercase().contains(&query))
            .map(|user| Person {
                login: user.login,
                name: user.full_name,
                id: Some(user.id),
            })
            .collect())
    }
    async fn set_assignees(
        &self,
        id: &ChangeRequestId,
        logins: &[String],
    ) -> Result<Vec<Person>, ForgeError> {
        let updated = self.update_change_request(
            id,
            &RequestPatch {
                assignees: Some(logins.to_vec()),
                ..RequestPatch::default()
            },
        )
        .await?;
        Ok(updated.assignees)
    }
    async fn list_milestones(&self, repository: &str) -> Result<Vec<Milestone>, ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .get(self.api(&format!(
                "repos/{repository}/milestones?state=open&limit=100"
            )))
            .header("Authorization", format!("token {token}"))
            .send()
            .await
            .map_err(network)?;
        let rows: Vec<MilestoneRow> = ensure(response).await?.json().await.map_err(network)?;
        Ok(rows
            .into_iter()
            .map(|row| Milestone { name: row.title })
            .collect())
    }
    async fn set_milestone(
        &self,
        id: &ChangeRequestId,
        milestone: Option<&str>,
    ) -> Result<Option<String>, ForgeError> {
        let updated = self.update_change_request(
            id,
            &RequestPatch {
                milestone: Some(milestone.map(str::to_owned)),
                ..RequestPatch::default()
            },
        )
        .await?;
        Ok(updated.milestone)
    }
    async fn merge_change_request(
        &self,
        id: &ChangeRequestId,
        strategy: MergeStrategy,
    ) -> Result<MergeOutcome, ForgeError> {
        let token = self.credential().await?;
        let do_ = match strategy {
            MergeStrategy::MergeCommit => "merge",
            MergeStrategy::Squash => "squash",
            MergeStrategy::Rebase => "rebase",
        };
        let response = reqwest::Client::new()
            .post(self.api(&format!(
                "repos/{}/pulls/{}/merge",
                id.repository, id.number
            )))
            .header("Authorization", format!("token {token}"))
            .json(&serde_json::json!({"Do": do_}))
            .send()
            .await
            .map_err(network)?;
        let status = response.status().as_u16();
        if status == 405 || status == 409 {
            return Err(ForgeError::Validation(
                "Forgejo rejected the merge (not mergeable or head changed)".into(),
            ));
        }
        ensure(response).await?;
        // The merge endpoint returns an empty body; refresh for the merge commit.
        Ok(MergeOutcome::default())
    }
    async fn delete_branch(&self, repository: &str, branch: &str) -> Result<(), ForgeError> {
        let token = self.credential().await?;
        let encoded =
            url::form_urlencoded::byte_serialize(branch.as_bytes()).collect::<String>();
        let response = reqwest::Client::new()
            .delete(self.api(&format!("repos/{repository}/branches/{encoded}")))
            .header("Authorization", format!("token {token}"))
            .send()
            .await
            .map_err(network)?;
        ensure(response).await.map(|_| ())
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
    #[serde(default)]
    body: Option<String>,
    user: User,
    head: Branch,
    base: Branch,
    #[serde(default)]
    draft: bool,
    updated_at: DateTime<Utc>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    merged: Option<bool>,
    #[serde(default)]
    merged_at: Option<DateTime<Utc>>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    mergeable: Option<bool>,
    #[serde(default)]
    labels: Vec<LabelRow>,
    #[serde(default)]
    assignees: Vec<User>,
    #[serde(default)]
    milestone: Option<MilestoneRow>,
    #[serde(default)]
    requested_reviewers: Vec<User>,
    #[serde(default)]
    head_sha: Option<String>,
    #[serde(default)]
    merge_commit_sha: Option<String>,
}
#[derive(Deserialize)]
struct User {
    login: String,
    #[serde(default)]
    id: u64,
    #[serde(default)]
    full_name: Option<String>,
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
                name: self.user.full_name.clone(),
                id: Some(self.user.id),
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
#[derive(Deserialize)]
struct LabelRow {
    id: u64,
    name: String,
    #[serde(default)]
    color: Option<String>,
}
#[derive(Deserialize)]
struct MilestoneRow {
    id: u64,
    title: String,
}
#[derive(Deserialize)]
struct RepositoryRow {
    #[serde(default)]
    default_branch: Option<String>,
}
fn request_state(row: &Row) -> RequestState {
    if row.merged.unwrap_or(false) || row.merged_at.is_some() {
        RequestState::Merged
    } else {
        match row.state.as_deref() {
            Some("closed") => RequestState::Closed,
            _ => RequestState::Open,
        }
    }
}
fn normalize(forge: &str, repo: &str, row: Row) -> ChangeRequest {
    let mut request = normalized_request(
        forge.into(),
        repo.into(),
        row.number,
        ChangeRequestKind::PullRequest,
        row.title.clone(),
        row.user.login.clone(),
        row.head.branch.clone(),
        row.base.branch.clone(),
        row.draft,
        row.updated_at,
    );
    request.body = row.body.clone().filter(|body| !body.is_empty());
    request.state = request_state(&row);
    request.labels = row
        .labels
        .iter()
        .map(|label| Label {
            name: label.name.clone(),
            color: label.color.clone(),
        })
        .collect();
    request.assignees = row
        .assignees
        .iter()
        .map(|user| Person {
            login: user.login.clone(),
            name: user.full_name.clone(),
            id: Some(user.id),
        })
        .collect();
    request.milestone = row.milestone.as_ref().map(|milestone| milestone.title.clone());
    request.web_url = row.html_url.clone();
    request.mergeable_state = None;
    request.head_sha = row.head_sha.clone();
    request.merged_sha = row.merge_commit_sha.clone();
    request.mergeability = match row.mergeable {
        Some(true) => crate::model::Mergeability::Mergeable,
        Some(false) => crate::model::Mergeability::Conflicting,
        None => crate::model::Mergeability::Unknown,
    };
    request.reviewers = row
        .requested_reviewers
        .iter()
        .map(|user| Reviewer {
            person: Person {
                login: user.login.clone(),
                name: user.full_name.clone(),
                id: Some(user.id),
            },
            state: ReviewState::Requested,
        })
        .collect();
    request
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
        let row: IssueComment = serde_json::from_str(r#"{"id":7,"body":"done","user":{"login":"jack","id":1},"created_at":"2026-08-29T12:00:00Z","updated_at":null,"html_url":null}"#).unwrap();
        assert_eq!(row.into_comment().id, "7");
    }
    #[test]
    fn normalizes_full_pull_metadata() {
        let row: Row = serde_json::from_str(r#"{"number":12,"title":"New renderer","body":"Body","user":{"login":"jack","id":1},"head":{"ref":"new"},"base":{"ref":"main"},"state":"open","updated_at":"2026-08-29T12:00:00Z","html_url":"https://codeberg.org/jack/foo/pulls/12","mergeable":true,"labels":[{"id":2,"name":"bug","color":"ff0000"}],"assignees":[{"login":"alice","id":2,"full_name":"Alice"}],"milestone":{"id":3,"title":"v1.2"},"requested_reviewers":[{"login":"bob","id":4}],"head_sha":"abc123"}"#).unwrap();
        let item = normalize("codeberg", "jack/foo", row);
        assert_eq!(item.state, RequestState::Open);
        assert_eq!(item.body.as_deref(), Some("Body"));
        assert_eq!(item.labels[0].name, "bug");
        assert_eq!(item.assignees[0].login, "alice");
        assert_eq!(item.milestone.as_deref(), Some("v1.2"));
        assert_eq!(item.reviewers[0].person.login, "bob");
        assert_eq!(item.head_sha.as_deref(), Some("abc123"));
    }

    #[test]
    fn merged_pull_maps_to_merged_state() {
        let row: Row = serde_json::from_str(r#"{"number":12,"title":"x","user":{"login":"jack"},"head":{"ref":"a"},"base":{"ref":"main"},"state":"closed","merged":true,"merged_at":"2026-08-29T13:00:00Z","merge_commit_sha":"def","updated_at":"2026-08-29T12:00:00Z"}"#).unwrap();
        let request = normalize("codeberg", "jack/foo", row);
        assert_eq!(request.state, RequestState::Merged);
        assert_eq!(request.merged_sha.as_deref(), Some("def"));
    }

    #[test]
    fn capabilities_keep_unsupported_forgejo_writes_hidden() {
        let caps = ForgeCapabilities {
            create_change_request: true,
            labels: true,
            merge: true,
            merge_commit: true,
            squash_merge: true,
            rebase_merge: true,
            ..ForgeCapabilities::default()
        };
        assert!(!caps.auto_merge);
        assert!(!caps.draft_transition);
        assert!(!caps.ci_read);
    }
}
