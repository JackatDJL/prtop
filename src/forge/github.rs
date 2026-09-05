use crate::{
    config::ProjectConfig,
    forge::{ForgeCapabilities, ForgeError, ForgeProvider, ReviewAction, auth, normalized_request},
    model::{
        ChangeRequest, ChangeRequestId, ChangeRequestKind, Comment, Job, JobId, Person, Pipeline,
        PipelineId, PipelineStatus,
    },
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
            ci_read: true,
            // Job-log download is a ZIP archive in GitHub's REST API. Do not advertise it
            // until the archive reader is in place.
            ci_logs: false,
            ci_retry_pipeline: true,
            ci_cancel_pipeline: true,
            ..ForgeCapabilities::default()
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
    async fn list_pipelines(&self, id: &ChangeRequestId) -> Result<Vec<Pipeline>, ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .get(self.api(&format!(
                "repos/{}/actions/runs?event=pull_request&per_page=100",
                id.repository
            )))
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "prtop")
            .send()
            .await
            .map_err(network)?;
        let runs: WorkflowRuns = ensure(response).await?.json().await.map_err(network)?;
        Ok(runs
            .workflow_runs
            .into_iter()
            .filter(|run| {
                run.pull_requests
                    .iter()
                    .any(|pull| pull.number == id.number)
            })
            .map(|run| github_pipeline(&self.name, &id.repository, run))
            .collect())
    }
    async fn get_pipeline(&self, id: &PipelineId) -> Result<Pipeline, ForgeError> {
        let token = self.credential().await?;
        let client = reqwest::Client::new();
        let mut page = 1;
        let mut all_jobs = vec![];
        loop {
            let response = client
                .get(self.api(&format!(
                    "repos/{}/actions/runs/{}/jobs?per_page=100&page={page}",
                    id.repository, id.value
                )))
                .header("Authorization", format!("Bearer {token}"))
                .header("User-Agent", "prtop")
                .send()
                .await
                .map_err(network)?;
            let jobs: WorkflowJobs = ensure(response).await?.json().await.map_err(network)?;
            let count = jobs.jobs.len();
            all_jobs.extend(jobs.jobs);
            if count < 100 {
                break;
            }
            page += 1;
        }
        Ok(Pipeline {
            id: id.clone(),
            name: format!("workflow #{}", id.value),
            ref_name: String::new(),
            sha: String::new(),
            status: PipelineStatus::Unknown,
            created_at: Utc::now(),
            started_at: None,
            finished_at: None,
            stages: vec![],
            jobs: all_jobs
                .into_iter()
                .map(|job| github_job(id, job))
                .collect(),
            url: None,
            environment: None,
        })
    }
    async fn retry_pipeline(&self, id: &PipelineId) -> Result<(), ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .post(self.api(&format!(
                "repos/{}/actions/runs/{}/rerun-failed-jobs",
                id.repository, id.value
            )))
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "prtop")
            .send()
            .await
            .map_err(network)?;
        ensure(response).await.map(|_| ())
    }
    async fn cancel_pipeline(&self, id: &PipelineId) -> Result<(), ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .post(self.api(&format!(
                "repos/{}/actions/runs/{}/cancel",
                id.repository, id.value
            )))
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "prtop")
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
#[derive(Deserialize)]
struct WorkflowRuns {
    workflow_runs: Vec<WorkflowRun>,
}
#[derive(Deserialize)]
struct WorkflowRun {
    id: u64,
    name: Option<String>,
    head_branch: Option<String>,
    head_sha: String,
    status: Option<String>,
    conclusion: Option<String>,
    created_at: DateTime<Utc>,
    run_started_at: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
    html_url: Option<String>,
    #[serde(default)]
    pull_requests: Vec<AssociatedPull>,
}
#[derive(Deserialize)]
struct AssociatedPull {
    number: u64,
}
#[derive(Deserialize)]
struct WorkflowJobs {
    jobs: Vec<WorkflowJob>,
}
#[derive(Deserialize)]
struct WorkflowJob {
    id: u64,
    name: String,
    status: Option<String>,
    conclusion: Option<String>,
    started_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
    html_url: Option<String>,
}
fn github_status(status: Option<&str>, conclusion: Option<&str>) -> PipelineStatus {
    match conclusion.or(status) {
        Some("success") => PipelineStatus::Success,
        Some("failure") => PipelineStatus::Failed,
        Some("cancelled") => PipelineStatus::Cancelled,
        Some("skipped") => PipelineStatus::Skipped,
        Some("timed_out") => PipelineStatus::TimedOut,
        Some("in_progress") => PipelineStatus::Running,
        Some("queued") => PipelineStatus::Queued,
        Some("waiting") => PipelineStatus::Waiting,
        _ => PipelineStatus::Unknown,
    }
}
fn github_pipeline(forge: &str, repo: &str, row: WorkflowRun) -> Pipeline {
    let id = PipelineId {
        forge: forge.into(),
        repository: repo.into(),
        value: row.id.to_string(),
    };
    let status = github_status(row.status.as_deref(), row.conclusion.as_deref());
    Pipeline {
        id,
        name: row.name.unwrap_or_else(|| "workflow".into()),
        ref_name: row.head_branch.unwrap_or_default(),
        sha: row.head_sha,
        status,
        created_at: row.created_at,
        started_at: row.run_started_at,
        finished_at: (!status.is_active()).then_some(row.updated_at),
        stages: vec![],
        jobs: vec![],
        url: row.html_url,
        environment: None,
    }
}
fn github_job(pipeline: &PipelineId, row: WorkflowJob) -> Job {
    let duration_seconds = row
        .started_at
        .zip(row.completed_at)
        .map(|(start, end)| (end - start).num_seconds().max(0) as u64);
    Job {
        id: JobId {
            pipeline: pipeline.clone(),
            value: row.id.to_string(),
        },
        name: row.name,
        stage: None,
        status: github_status(row.status.as_deref(), row.conclusion.as_deref()),
        started_at: row.started_at,
        finished_at: row.completed_at,
        duration_seconds,
        runner: None,
        attempt: 1,
        allow_failure: false,
        url: row.html_url,
        environment: None,
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

    #[test]
    fn keeps_only_workflow_runs_for_the_requested_pull() {
        let runs: WorkflowRuns = serde_json::from_str(r#"{"workflow_runs":[{"id":1,"name":"one","head_sha":"a","status":"completed","conclusion":"success","created_at":"2026-08-29T12:00:00Z","updated_at":"2026-08-29T12:01:00Z","pull_requests":[{"number":4}]},{"id":2,"name":"two","head_sha":"b","status":"completed","conclusion":"failure","created_at":"2026-08-29T12:00:00Z","updated_at":"2026-08-29T12:01:00Z","pull_requests":[{"number":5}]}]}"#).unwrap();
        assert_eq!(
            runs.workflow_runs
                .into_iter()
                .filter(|run| run.pull_requests.iter().any(|pull| pull.number == 4))
                .count(),
            1
        );
    }

    #[test]
    fn active_workflow_has_no_finished_time() {
        let run: WorkflowRun = serde_json::from_str(r#"{"id":1,"head_sha":"a","status":"in_progress","created_at":"2026-08-29T12:00:00Z","updated_at":"2026-08-29T12:01:00Z"}"#).unwrap();
        assert!(
            github_pipeline("github", "jack/prtop", run)
                .finished_at
                .is_none()
        );
    }
}
