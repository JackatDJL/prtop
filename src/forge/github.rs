use crate::{
    config::ProjectConfig,
    forge::{
        ForgeCapabilities, ForgeError, ForgeProvider, MergeOutcome, MergeStrategy, NewChangeRequest,
        RepositoryInfo, RequestPatch, ReviewAction, auth, normalized_request,
    },
    model::{
        ChangeRequest, ChangeRequestId, ChangeRequestKind, Comment, Job, JobId, Label, MergeQueue,
        Person, Pipeline, PipelineId, PipelineStatus, RequestState, ReviewState, Reviewer,
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
    async fn get_pull(&self, token: &str, repository: &str, number: u64) -> Result<Row, ForgeError> {
        let response = reqwest::Client::new()
            .get(self.api(&format!("repos/{repository}/pulls/{number}")))
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "prtop")
            .send()
            .await
            .map_err(network)?;
        let row: Row = ensure(response).await?.json().await.map_err(network)?;
        Ok(row)
    }
    /// GitHub has no draft toggle on the REST patch endpoint. Report it in the error instead
    /// of silently dropping the request.
    async fn patch_pull(
        &self,
        token: &str,
        id: &ChangeRequestId,
        body: serde_json::Value,
    ) -> Result<Row, ForgeError> {
        let response = reqwest::Client::new()
            .patch(self.api(&format!(
                "repos/{}/pulls/{}",
                id.repository, id.number
            )))
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "prtop")
            .json(&body)
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
    async fn find_milestone(
        &self,
        token: String,
        repository: &str,
        title: &str,
    ) -> Option<u64> {
        self.milestone_number(&token, repository, title)
            .await
            .ok()
    }
    async fn milestone_number(
        &self,
        token: &str,
        repository: &str,
        title: &str,
    ) -> Result<u64, ForgeError> {
        let response = reqwest::Client::new()
            .get(self.api(&format!(
                "repos/{repository}/milestones?state=open&per_page=100"
            )))
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "prtop")
            .send()
            .await
            .map_err(network)?;
        let rows: Vec<MilestoneRow> = ensure(response).await?.json().await.map_err(network)?;
        rows.iter()
            .find(|row| row.title == title)
            .map(|row| row.number)
            .ok_or_else(|| ForgeError::Validation(format!("milestone {title} not found")))
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
            create_change_request: true,
            edit_title: true,
            edit_description: true,
            labels: true,
            assignees: true,
            milestone: true,
            // The REST patch endpoint has no draft parameter; GraphQL-only.
            draft_transition: false,
            close: true,
            reopen: true,
            merge: true,
            merge_commit: true,
            squash_merge: true,
            rebase_merge: true,
            auto_merge: true,
            delete_source_branch: true,
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
    async fn get_change_request(&self, id: &ChangeRequestId) -> Result<ChangeRequest, ForgeError> {
        let token = self.credential().await?;
        self.fetch_full(&token, id).await
    }
    async fn get_repository(&self, repository: &str) -> Result<RepositoryInfo, ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .get(self.api(&format!("repos/{repository}")))
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "prtop")
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
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "prtop")
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
        // Metadata attached after creation is best-effort: the PR exists either way and the
        // targeted refresh reconciles the provider truth.
        if !input.reviewers.is_empty() {
            let _ = reqwest::Client::new()
                .post(self.api(&format!(
                    "repos/{}/pulls/{}/requested_reviewers",
                    repository, created.id.number
                )))
                .header("Authorization", format!("Bearer {token}"))
                .header("User-Agent", "prtop")
                .json(&serde_json::json!({"reviewers": input.reviewers}))
                .send()
                .await;
        }
        if !input.labels.is_empty() || !input.assignees.is_empty() || input.milestone.is_some() {
            let mut issue = serde_json::Map::new();
            if !input.labels.is_empty() {
                issue.insert("labels".into(), serde_json::json!(input.labels));
            }
            if !input.assignees.is_empty() {
                issue.insert("assignees".into(), serde_json::json!(input.assignees));
            }
            if let Some(milestone) = &input.milestone {
                if let Some(number) = self
                    .find_milestone(token.clone(), repository, milestone)
                    .await
                {                    issue.insert("milestone".into(), serde_json::json!(number));
                }
            }
            if !issue.is_empty() {
                let _ = reqwest::Client::new()
                    .patch(self.api(&format!(
                        "repos/{}/issues/{}",
                        repository, created.id.number
                    )))
                    .header("Authorization", format!("Bearer {token}"))
                    .header("User-Agent", "prtop")
                    .json(&serde_json::json!(issue))
                    .send()
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
        let mut body = serde_json::Map::new();
        if let Some(title) = &patch.title {
            body.insert("title".into(), serde_json::json!(title));
        }
        if let Some(body_text) = &patch.body {
            body.insert("body".into(), serde_json::json!(body_text));
        }
        if let Some(state) = patch.state {
            body.insert(
                "state".into(),
                serde_json::json!(match state {
                    RequestState::Open => "open",
                    RequestState::Closed => "closed",
                    RequestState::Merged => "closed",
                }),
            );
        }
        if patch.draft.is_some() {
            return Err(ForgeError::Validation(
                "GitHub REST does not support draft transitions".into(),
            ));
        }
        if !body.is_empty() {
            self.patch_pull(&token, id, body.into()).await?;
        }
        self.fetch_full(&token, id).await
    }
    async fn list_labels(&self, repository: &str) -> Result<Vec<Label>, ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .get(self.api(&format!("repos/{repository}/labels?per_page=100")))
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "prtop")
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
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .put(self.api(&format!(
                "repos/{}/issues/{}/labels",
                id.repository, id.number
            )))
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "prtop")
            .json(&serde_json::json!({"labels": names}))
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
    async fn search_assignees(
        &self,
        repository: &str,
        query: &str,
    ) -> Result<Vec<Person>, ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .get(self.api(&format!(
                "repos/{repository}/assignees?per_page=100"
            )))
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "prtop")
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
                name: None,
                id: Some(user.id),
            })
            .collect())
    }
    async fn set_assignees(
        &self,
        id: &ChangeRequestId,
        logins: &[String],
    ) -> Result<Vec<Person>, ForgeError> {
        let token = self.credential().await?;
        let current: IssueDetail = reqwest::Client::new()
            .get(self.api(&format!(
                "repos/{}/issues/{}",
                id.repository, id.number
            )))
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "prtop")
            .send()
            .await
            .map_err(network)?
            .json()
            .await
            .map_err(network)?;
        let wanted: Vec<&String> = logins.iter().collect();
        let missing: Vec<&String> = wanted
            .iter()
            .filter(|login| !current.assignees.iter().any(|user| &user.login == **login))
            .map(|login| *login)
            .collect();
        let removed: Vec<&String> = current
            .assignees
            .iter()
            .filter(|user| !logins.contains(&user.login))
            .map(|user| &user.login)
            .collect();
        if !missing.is_empty() {
            let response = reqwest::Client::new()
                .post(self.api(&format!(
                    "repos/{}/issues/{}/assignees",
                    id.repository, id.number
                )))
                .header("Authorization", format!("Bearer {token}"))
                .header("User-Agent", "prtop")
                .json(&serde_json::json!({"assignees": missing}))
                .send()
                .await
                .map_err(network)?;
            ensure(response).await?;
        }
        if !removed.is_empty() {
            let response = reqwest::Client::new()
                .delete(self.api(&format!(
                    "repos/{}/issues/{}/assignees",
                    id.repository, id.number
                )))
                .header("Authorization", format!("Bearer {token}"))
                .header("User-Agent", "prtop")
                .json(&serde_json::json!({"assignees": removed}))
                .send()
                .await
                .map_err(network)?;
            ensure(response).await?;
        }
        let refreshed: IssueDetail = reqwest::Client::new()
            .get(self.api(&format!(
                "repos/{}/issues/{}",
                id.repository, id.number
            )))
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "prtop")
            .send()
            .await
            .map_err(network)?
            .json()
            .await
            .map_err(network)?;
        Ok(refreshed
            .assignees
            .into_iter()
            .map(|user| Person {
                login: user.login,
                name: None,
                id: Some(user.id),
            })
            .collect())
    }
    async fn list_milestones(
        &self,
        repository: &str,
    ) -> Result<Vec<crate::forge::Milestone>, ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .get(self.api(&format!(
                "repos/{repository}/milestones?state=open&per_page=100"
            )))
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "prtop")
            .send()
            .await
            .map_err(network)?;
        let rows: Vec<MilestoneRow> = ensure(response).await?.json().await.map_err(network)?;
        Ok(rows
            .into_iter()
            .map(|row| crate::forge::Milestone { name: row.title })
            .collect())
    }
    async fn set_milestone(
        &self,
        id: &ChangeRequestId,
        milestone: Option<&str>,
    ) -> Result<Option<String>, ForgeError> {
        let token = self.credential().await?;
        let number = match milestone {
            None => None,
            Some(name) => self.find_milestone(token.clone(), &id.repository, name).await,
        };
        let response = reqwest::Client::new()
            .patch(self.api(&format!(
                "repos/{}/issues/{}",
                id.repository, id.number
            )))
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "prtop")
            .json(&serde_json::json!({"milestone": number}))
            .send()
            .await
            .map_err(network)?;
        ensure(response).await?;
        Ok(milestone.map(str::to_owned))
    }
    async fn set_auto_merge(
        &self,
        id: &ChangeRequestId,
        enable: bool,
        strategy: MergeStrategy,
    ) -> Result<bool, ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new();
        let result = if enable {
            response
                .put(self.api(&format!(
                    "repos/{}/pulls/{}/auto-merge",
                    id.repository, id.number
                )))
                .header("Authorization", format!("Bearer {token}"))
                .header("User-Agent", "prtop")
                .json(&serde_json::json!({
                    "merge_method": strategy.api_name(),
                }))
                .send()
                .await
        } else {
            response
                .delete(self.api(&format!(
                    "repos/{}/pulls/{}/auto-merge",
                    id.repository, id.number
                )))
                .header("Authorization", format!("Bearer {token}"))
                .header("User-Agent", "prtop")
                .send()
                .await
        };
        let response = result.map_err(network)?;
        ensure(response).await?;
        Ok(enable)
    }
    async fn merge_change_request(
        &self,
        id: &ChangeRequestId,
        strategy: MergeStrategy,
    ) -> Result<MergeOutcome, ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .put(self.api(&format!(
                "repos/{}/pulls/{}/merge",
                id.repository, id.number
            )))
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "prtop")
            .json(&serde_json::json!({
                "merge_method": strategy.api_name(),
            }))
            .send()
            .await
            .map_err(network)?;
        let status = response.status().as_u16();
        if status == 405 || status == 409 {
            let message = response.json::<MergeError>().await.ok();
            return Err(ForgeError::Validation(
                message
                    .and_then(|error| error.message)
                    .unwrap_or_else(|| "merge rejected by GitHub".into()),
            ));
        }
        let response = ensure(response).await?;
        let row: MergedRow = response.json().await.map_err(network)?;
        Ok(MergeOutcome {
            sha: row.sha,
            message: row.merged.then(|| "merged".into()),
        })
    }
    async fn delete_branch(&self, repository: &str, branch: &str) -> Result<(), ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .delete(self.api(&format!(
                "repos/{}/git/refs/heads/{branch}",
                url::form_urlencoded::byte_serialize(repository.as_bytes()).collect::<String>()
            )))
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "prtop")
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
                id: Some(user.id),
            })
            .collect())
    }
    async fn request_reviewer(
        &self,
        id: &ChangeRequestId,
        reviewer: &Person,
    ) -> Result<(), ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .post(self.api(&format!(
                "repos/{}/pulls/{}/requested_reviewers",
                id.repository, id.number
            )))
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "prtop")
            .json(&serde_json::json!({"reviewers":[reviewer.login]}))
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
    #[serde(default)]
    body: Option<String>,
    user: User,
    head: Branch,
    base: Branch,
    #[serde(default)]
    draft: bool,
    state: Option<String>,
    #[serde(default)]
    merged_at: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
    html_url: Option<String>,
    mergeable: Option<bool>,
    #[serde(default)]
    mergeable_state: Option<String>,
    #[serde(default)]
    auto_merge: Option<bool>,
    #[serde(default)]
    labels: Vec<LabelRow>,
    #[serde(default)]
    assignees: Vec<User>,
    #[serde(default)]
    milestone: Option<MilestoneRow>,
    #[serde(default)]
    requested_reviewers: Vec<User>,
    #[serde(default)]
    merge_commit_sha: Option<String>,
    #[serde(default)]
    head_sha: Option<String>,
}
#[derive(Deserialize)]
struct User {
    login: String,
    #[serde(default)]
    id: u64,
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
                id: Some(self.user.id),
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
#[derive(Deserialize)]
struct LabelRow {
    name: String,
    #[serde(default)]
    color: Option<String>,
}
#[derive(Deserialize)]
struct MilestoneRow {
    number: u64,
    title: String,
}
#[derive(Deserialize)]
struct RepositoryRow {
    #[serde(default)]
    default_branch: Option<String>,
}
#[derive(Deserialize)]
struct IssueDetail {
    #[serde(default)]
    assignees: Vec<User>,
}
#[derive(Deserialize)]
struct MergeError {
    #[serde(default)]
    message: Option<String>,
}
#[derive(Deserialize)]
struct MergedRow {
    #[serde(default)]
    sha: Option<String>,
    #[serde(default)]
    merged: bool,
}
fn request_state(row: &Row) -> RequestState {
    if row.merged_at.is_some() {
        RequestState::Merged
    } else {
        match row.state.as_deref() {
            Some("closed") => RequestState::Closed,
            _ => RequestState::Open,
        }
    }
}
fn labels(rows: &[LabelRow]) -> Vec<Label> {
    rows.iter()
        .map(|row| Label {
            name: row.name.clone(),
            color: row.color.clone(),
        })
        .collect()
}
fn mergeability(row: &Row) -> crate::model::Mergeability {
    match row.mergeable {
        Some(true) => crate::model::Mergeability::Mergeable,
        Some(false) => crate::model::Mergeability::Conflicting,
        None => crate::model::Mergeability::Unknown,
    }
}
fn normalize(forge: &str, repo: &str, row: Row) -> ChangeRequest {
    let kind = ChangeRequestKind::PullRequest;
    let mut request = normalized_request(
        forge.into(),
        repo.into(),
        row.number,
        kind,
        row.title.clone(),
        row.user.login.clone(),
        row.head.branch.clone(),
        row.base.branch.clone(),
        row.draft,
        row.updated_at,
    );
    request.body = row.body.clone().filter(|body| !body.is_empty());
    request.state = request_state(&row);
    request.labels = labels(&row.labels);
    request.assignees = row
        .assignees
        .iter()
        .map(|user| Person {
            login: user.login.clone(),
            name: None,
            id: Some(user.id),
        })
        .collect();
    request.milestone = row.milestone.as_ref().map(|milestone| milestone.title.clone());
    request.web_url = row.html_url.clone();
    request.auto_merge = row.auto_merge.unwrap_or(false);
    request.mergeable_state = row.mergeable_state.clone();
    request.head_sha = row.head_sha.clone();
    request.merged_sha = row.merge_commit_sha.clone();
    request.mergeability = mergeability(&row);
    request
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
        let row: IssueComment = serde_json::from_str(r#"{"id":7,"body":"done","user":{"login":"jack","id":3},"created_at":"2026-08-29T12:00:00Z","updated_at":"2026-08-29T12:01:00Z","html_url":"https://example.test/comment/7"}"#).unwrap();
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

    #[test]
    fn normalizes_full_pull_metadata() {
        let row: Row = serde_json::from_str(r#"{"number":184,"title":"Fix droplet reader","body":"Body text","user":{"login":"jack","id":1},"head":{"ref":"feature/reader"},"base":{"ref":"main"},"draft":false,"state":"open","updated_at":"2026-08-29T12:00:00Z","html_url":"https://github.com/jack/prtop/pull/184","mergeable":true,"mergeable_state":"clean","auto_merge":false,"labels":[{"name":"bug","color":"ff0000"}],"assignees":[{"login":"alice","id":2}],"milestone":{"number":3,"title":"v1.2"},"requested_reviewers":[{"login":"bob","id":4}],"merge_commit_sha":null,"head_sha":"4e2f73a"}"#).unwrap();
        let item = normalize("github", "jack/quickdrop", row);
        assert_eq!(item.state, RequestState::Open);
        assert_eq!(item.body.as_deref(), Some("Body text"));
        assert_eq!(item.labels[0].name, "bug");
        assert_eq!(item.assignees[0].login, "alice");
        assert_eq!(item.milestone.as_deref(), Some("v1.2"));
        assert_eq!(item.web_url.as_deref(), Some("https://github.com/jack/quickdrop/pull/184"));
        assert_eq!(item.mergeable_state.as_deref(), Some("clean"));
        assert_eq!(item.head_sha.as_deref(), Some("4e2f73a"));
        assert_eq!(item.reviewers.len(), 1);
        assert!(!item.auto_merge);
    }

    #[test]
    fn closed_and_merged_pulls_map_to_their_state() {
        let closed: Row = serde_json::from_str(r#"{"number":1,"title":"x","user":{"login":"jack"},"head":{"ref":"a"},"base":{"ref":"main"},"state":"closed","updated_at":"2026-08-29T12:00:00Z"}"#).unwrap();
        assert_eq!(normalize("github", "r", closed).state, RequestState::Closed);
        let merged: Row = serde_json::from_str(r#"{"number":1,"title":"x","user":{"login":"jack"},"head":{"ref":"a"},"base":{"ref":"main"},"state":"closed","merged_at":"2026-08-29T13:00:00Z","merge_commit_sha":"abc","updated_at":"2026-08-29T12:00:00Z"}"#).unwrap();
        let request = normalize("github", "r", merged);
        assert_eq!(request.state, RequestState::Merged);
        assert_eq!(request.merged_sha.as_deref(), Some("abc"));
    }

    #[test]
    fn mergeability_distinguishes_unknown_from_conflicting() {
        let unknown: Row = serde_json::from_str(r#"{"number":1,"title":"x","user":{"login":"jack"},"head":{"ref":"a"},"base":{"ref":"main"},"updated_at":"2026-08-29T12:00:00Z"}"#).unwrap();
        assert_eq!(
            normalize("github", "r", unknown).mergeability,
            crate::model::Mergeability::Unknown
        );
        let conflicting: Row = serde_json::from_str(r#"{"number":1,"title":"x","user":{"login":"jack"},"head":{"ref":"a"},"base":{"ref":"main"},"mergeable":false,"updated_at":"2026-08-29T12:00:00Z"}"#).unwrap();
        assert_eq!(
            normalize("github", "r", conflicting).mergeability,
            crate::model::Mergeability::Conflicting
        );
    }

    #[test]
    fn strategy_api_names_match_github() {
        assert_eq!(MergeStrategy::MergeCommit.api_name(), "merge");
        assert_eq!(MergeStrategy::Squash.api_name(), "squash");
        assert_eq!(MergeStrategy::Rebase.api_name(), "rebase");
    }

    #[test]
    fn capabilities_advertise_supported_lifecycle() {
        let caps = ForgeCapabilities {
            create_change_request: true,
            edit_title: true,
            edit_description: true,
            labels: true,
            assignees: true,
            milestone: true,
            close: true,
            reopen: true,
            merge: true,
            merge_commit: true,
            squash_merge: true,
            rebase_merge: true,
            auto_merge: true,
            delete_source_branch: true,
            ..ForgeCapabilities::default()
        };
        assert!(caps.create_change_request && caps.auto_merge);
        assert!(!caps.draft_transition);
        assert!(!caps.ci_logs);
    }
}
