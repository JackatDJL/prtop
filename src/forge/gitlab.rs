use crate::{
    config::ProjectConfig,
    forge::{ForgeCapabilities, ForgeError, ForgeProvider, ReviewAction, auth, normalized_request},
    model::{
        ChangeRequest, ChangeRequestId, ChangeRequestKind, Comment, Job, JobId, LogChunk, Person,
        Pipeline, PipelineId, PipelineStage, PipelineStatus,
    },
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
    async fn credential(&self) -> Result<String, ForgeError> {
        match &self.token {
            Some(token) => Ok(token.clone()),
            None => {
                auth::resolve(
                    &["PRTOP_GITLAB_TOKEN", "GITLAB_TOKEN"],
                    "glab",
                    &["auth", "token"],
                    "run glab auth login or set PRTOP_GITLAB_TOKEN",
                )
                .await
            }
        }
    }
    fn project(id: &ChangeRequestId) -> String {
        url::form_urlencoded::byte_serialize(id.repository.as_bytes()).collect()
    }
    fn api(&self, path: &str) -> String {
        format!("https://{}/api/v4/{path}", self.host)
    }
}
#[async_trait]
impl ForgeProvider for GitLabProvider {
    fn name(&self) -> &str {
        &self.name
    }
    fn capabilities(&self) -> ForgeCapabilities {
        ForgeCapabilities {
            comments: true,
            reviews: true,
            approve: true,
            request_changes: false,
            request_reviewers: false,
            edit_comments: true,
            delete_comments: true,
            ci_read: true,
            ci_logs: true,
            ci_retry_job: true,
            ci_retry_pipeline: true,
            ci_cancel_job: true,
            ci_cancel_pipeline: true,
            ci_play_manual: true,
            ci_artifacts: true,
        }
    }
    async fn list_change_requests(&self) -> Result<Vec<ChangeRequest>, ForgeError> {
        let token = self.credential().await?;
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
                .header("PRIVATE-TOKEN", &token)
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
                "projects/{}/merge_requests/{}/notes",
                Self::project(id),
                id.number
            )))
            .header("PRIVATE-TOKEN", token)
            .json(&serde_json::json!({"body":body}))
            .send()
            .await
            .map_err(network)?;
        ensure(response).await.map(|_| ())
    }
    async fn edit_comment(
        &self,
        id: &ChangeRequestId,
        comment_id: &str,
        body: &str,
    ) -> Result<Comment, ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .put(self.api(&format!(
                "projects/{}/merge_requests/{}/notes/{comment_id}",
                Self::project(id),
                id.number
            )))
            .header("PRIVATE-TOKEN", token)
            .json(&serde_json::json!({"body":body}))
            .send()
            .await
            .map_err(network)?;
        let note: Note = ensure(response).await?.json().await.map_err(network)?;
        Ok(note.into_comment())
    }
    async fn delete_comment(
        &self,
        id: &ChangeRequestId,
        comment_id: &str,
    ) -> Result<(), ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .delete(self.api(&format!(
                "projects/{}/merge_requests/{}/notes/{comment_id}",
                Self::project(id),
                id.number
            )))
            .header("PRIVATE-TOKEN", token)
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
        match action {
            ReviewAction::Approve => {
                let token = self.credential().await?;
                let response = reqwest::Client::new()
                    .post(self.api(&format!(
                        "projects/{}/merge_requests/{}/approve",
                        Self::project(id),
                        id.number
                    )))
                    .header("PRIVATE-TOKEN", token)
                    .send()
                    .await
                    .map_err(network)?;
                ensure(response).await.map(|_| ())
            }
            ReviewAction::Comment => self.create_comment(id, body).await,
            ReviewAction::RequestChanges => Err(ForgeError::Unsupported),
        }
    }
    async fn search_reviewers(
        &self,
        _: &ChangeRequestId,
        query: &str,
    ) -> Result<Vec<Person>, ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .get(self.api(&format!(
                "users?search={}",
                url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>()
            )))
            .header("PRIVATE-TOKEN", token)
            .send()
            .await
            .map_err(network)?;
        let rows: Vec<User> = ensure(response).await?.json().await.map_err(network)?;
        Ok(rows
            .into_iter()
            .map(|user| Person {
                login: user.username,
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
        let response=reqwest::Client::new().put(self.api(&format!("projects/{}/merge_requests/{}",Self::project(id),id.number))).header("PRIVATE-TOKEN",token).json(&serde_json::json!({"reviewer_ids":[reviewer.parse::<u64>().map_err(|_|ForgeError::Validation("GitLab reviewer must be selected from search".into()))?]})).send().await.map_err(network)?;
        ensure(response).await.map(|_| ())
    }
    async fn list_pipelines(&self, id: &ChangeRequestId) -> Result<Vec<Pipeline>, ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .get(self.api(&format!(
                "projects/{}/merge_requests/{}/pipelines?per_page=100",
                Self::project(id),
                id.number
            )))
            .header("PRIVATE-TOKEN", token)
            .send()
            .await
            .map_err(network)?;
        let rows: Vec<PipelineRow> = ensure(response).await?.json().await.map_err(network)?;
        Ok(rows
            .into_iter()
            .map(|row| gitlab_pipeline(&self.name, &id.repository, row))
            .collect())
    }
    async fn get_pipeline(&self, id: &PipelineId) -> Result<Pipeline, ForgeError> {
        let token = self.credential().await?;
        let client = reqwest::Client::new();
        let project =
            url::form_urlencoded::byte_serialize(id.repository.as_bytes()).collect::<String>();
        let response = client
            .get(self.api(&format!("projects/{project}/pipelines/{}", id.value)))
            .header("PRIVATE-TOKEN", &token)
            .send()
            .await
            .map_err(network)?;
        let row: PipelineRow = ensure(response).await?.json().await.map_err(network)?;
        let response = client
            .get(self.api(&format!(
                "projects/{project}/pipelines/{}/jobs?per_page=100",
                id.value
            )))
            .header("PRIVATE-TOKEN", token)
            .send()
            .await
            .map_err(network)?;
        let rows: Vec<JobRow> = ensure(response).await?.json().await.map_err(network)?;
        let jobs = rows
            .into_iter()
            .map(|row| gitlab_job(id, row))
            .collect::<Vec<_>>();
        let stages = jobs.iter().filter_map(|job| job.stage.clone()).fold(
            Vec::<PipelineStage>::new(),
            |mut stages, name| {
                if !stages.iter().any(|stage| stage.name == name) {
                    stages.push(PipelineStage {
                        name,
                        status: PipelineStatus::Unknown,
                    });
                }
                stages
            },
        );
        let mut pipeline = gitlab_pipeline(&id.forge, &id.repository, row);
        pipeline.stages = stages;
        pipeline.jobs = jobs;
        Ok(pipeline)
    }
    async fn get_job_log(&self, id: &JobId, offset: usize) -> Result<LogChunk, ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .get(self.api(&format!(
                    "projects/{}/jobs/{}/trace",
                    url::form_urlencoded::byte_serialize(id.pipeline.repository.as_bytes())
                        .collect::<String>(),
                    id.value
                )))
            .header("PRIVATE-TOKEN", token)
            .send()
            .await
            .map_err(network)?;
        let text = ensure(response).await?.text().await.map_err(network)?;
        Ok(LogChunk {
            text: text.get(offset..).unwrap_or_default().to_owned(),
            complete: true,
        })
    }
    async fn retry_job(&self, id: &JobId) -> Result<(), ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .post(self.api(&format!(
                    "projects/{}/jobs/{}/retry",
                    url::form_urlencoded::byte_serialize(id.pipeline.repository.as_bytes())
                        .collect::<String>(),
                    id.value
                )))
            .header("PRIVATE-TOKEN", token)
            .send()
            .await
            .map_err(network)?;
        ensure(response).await.map(|_| ())
    }
    async fn retry_pipeline(&self, id: &PipelineId) -> Result<(), ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .post(self.api(&format!(
                "projects/{}/pipelines/{}/retry",
                url::form_urlencoded::byte_serialize(id.repository.as_bytes()).collect::<String>(),
                id.value
            )))
            .header("PRIVATE-TOKEN", token)
            .send()
            .await
            .map_err(network)?;
        ensure(response).await.map(|_| ())
    }
    async fn cancel_job(&self, id: &JobId) -> Result<(), ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .post(self.api(&format!(
                    "projects/{}/jobs/{}/cancel",
                    url::form_urlencoded::byte_serialize(id.pipeline.repository.as_bytes())
                        .collect::<String>(),
                    id.value
                )))
            .header("PRIVATE-TOKEN", token)
            .send()
            .await
            .map_err(network)?;
        ensure(response).await.map(|_| ())
    }
    async fn cancel_pipeline(&self, id: &PipelineId) -> Result<(), ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .post(self.api(&format!(
                "projects/{}/pipelines/{}/cancel",
                url::form_urlencoded::byte_serialize(id.repository.as_bytes()).collect::<String>(),
                id.value
            )))
            .header("PRIVATE-TOKEN", token)
            .send()
            .await
            .map_err(network)?;
        ensure(response).await.map(|_| ())
    }
    async fn play_job(&self, id: &JobId) -> Result<(), ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .post(self.api(&format!(
                    "projects/{}/jobs/{}/play",
                    url::form_urlencoded::byte_serialize(id.pipeline.repository.as_bytes())
                        .collect::<String>(),
                    id.value
                )))
            .header("PRIVATE-TOKEN", token)
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
            "GitLab authentication expired".into(),
        )),
        403 => Err(ForgeError::PermissionDenied),
        404 => Err(ForgeError::NotFound),
        409 => Err(ForgeError::Conflict),
        429 => Err(ForgeError::RateLimited {
            retry_after_seconds: None,
        }),
        _ => Err(ForgeError::Unavailable("GitLab request failed".into())),
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
#[derive(Deserialize)]
struct Note {
    id: u64,
    body: String,
    author: User,
    created_at: DateTime<Utc>,
    updated_at: Option<DateTime<Utc>>,
    web_url: Option<String>,
}
#[derive(Deserialize)]
struct PipelineRow {
    id: u64,
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "ref")]
    ref_: Option<String>,
    #[serde(default)]
    sha: String,
    status: String,
    #[serde(default)]
    created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    finished_at: Option<DateTime<Utc>>,
    #[serde(default)]
    web_url: Option<String>,
}
#[derive(Deserialize)]
struct JobRow {
    id: u64,
    name: String,
    stage: String,
    status: String,
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
    duration: Option<f64>,
    web_url: Option<String>,
    #[serde(default)]
    allow_failure: bool,
}
fn gitlab_status(value: &str) -> PipelineStatus {
    match value {
        "success" => PipelineStatus::Success,
        "failed" => PipelineStatus::Failed,
        "running" => PipelineStatus::Running,
        "pending" | "created" | "preparing" | "scheduled" | "waiting_for_callback" => {
            PipelineStatus::Pending
        }
        "canceling" => PipelineStatus::Running,
        "canceled" => PipelineStatus::Cancelled,
        "skipped" => PipelineStatus::Skipped,
        "manual" => PipelineStatus::Manual,
        "waiting_for_resource" => PipelineStatus::Waiting,
        _ => PipelineStatus::Unknown,
    }
}
fn gitlab_pipeline(forge: &str, repo: &str, row: PipelineRow) -> Pipeline {
    let id = PipelineId {
        forge: forge.into(),
        repository: repo.into(),
        value: row.id.to_string(),
    };
    Pipeline {
        id,
        name: row.name.unwrap_or_else(|| "pipeline".into()),
        ref_name: row.ref_.unwrap_or_default(),
        sha: row.sha,
        status: gitlab_status(&row.status),
        created_at: row.created_at.unwrap_or_else(Utc::now),
        started_at: row.started_at,
        finished_at: row.finished_at,
        stages: vec![],
        jobs: vec![],
        url: row.web_url,
        environment: None,
    }
}
fn gitlab_job(pipeline: &PipelineId, row: JobRow) -> Job {
    Job {
        id: JobId {
            pipeline: pipeline.clone(),
            value: row.id.to_string(),
        },
        name: row.name,
        stage: Some(row.stage),
        status: gitlab_status(&row.status),
        started_at: row.started_at,
        finished_at: row.finished_at,
        duration_seconds: row.duration.map(|value| value.max(0.0) as u64),
        runner: None,
        attempt: 1,
        allow_failure: row.allow_failure,
        url: row.web_url,
        environment: None,
    }
}
impl Note {
    fn into_comment(self) -> Comment {
        Comment {
            id: self.id.to_string(),
            author: Person {
                login: self.author.username,
                name: None,
            },
            body: self.body,
            created_at: self.created_at,
            updated_at: self.updated_at,
            can_edit: true,
            can_delete: true,
            url: self.web_url,
            resolved: None,
        }
    }
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
    #[test]
    fn normalizes_note_write_response() {
        let row: Note = serde_json::from_str(r#"{"id":7,"body":"done","author":{"username":"jack"},"created_at":"2026-08-29T12:00:00Z","updated_at":null,"web_url":null}"#).unwrap();
        assert_eq!(row.into_comment().id, "7");
    }

    #[test]
    fn merge_request_pipeline_rows_allow_absent_detail_timestamps() {
        let row: PipelineRow =
            serde_json::from_str(r#"{"id":7,"sha":"abc","status":"running","ref":"feature"}"#)
                .unwrap();
        let pipeline = gitlab_pipeline("work", "team/repo", row);
        assert_eq!(pipeline.status, PipelineStatus::Running);
        assert!(pipeline.started_at.is_none());
    }

    #[test]
    fn maps_active_gitlab_job_statuses() {
        assert_eq!(gitlab_status("scheduled"), PipelineStatus::Pending);
        assert_eq!(gitlab_status("canceling"), PipelineStatus::Running);
        assert_eq!(
            gitlab_status("waiting_for_callback"),
            PipelineStatus::Pending
        );
    }
}
