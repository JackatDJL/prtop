use crate::{
    config::ProjectConfig,
    forge::{
        ForgeCapabilities, ForgeError, ForgeProvider, MergeOutcome, MergeStrategy, Milestone,
        NewChangeRequest, RepositoryInfo, RequestPatch, ReviewAction, auth, normalized_request,
    },
    model::{
        ChangeRequest, ChangeRequestId, ChangeRequestKind, Comment, Job, JobId, Label, LogChunk,
        MergeQueue, Person, Pipeline, PipelineId, PipelineStage, PipelineStatus, RequestState,
        ReviewState, Reviewer,
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
const DRAFT_PREFIX: &str = "Draft: ";
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
    async fn get_mr(
        &self,
        token: &str,
        id: &ChangeRequestId,
    ) -> Result<ChangeRequest, ForgeError> {
        let response = reqwest::Client::new()
            .get(self.api(&format!(
                "projects/{}/merge_requests/{}",
                Self::project(id),
                id.number
            )))
            .header("PRIVATE-TOKEN", token)
            .send()
            .await
            .map_err(network)?;
        let row: Row = ensure(response).await?.json().await.map_err(network)?;
        Ok(normalize(&self.name, &id.repository, row))
    }
    async fn user_ids(
        &self,
        token: &str,
        project: &str,
        logins: &[String],
    ) -> Result<Vec<u64>, ForgeError> {
        if logins.is_empty() {
            return Ok(vec![]);
        }
        let response = reqwest::Client::new()
            .get(self.api(&format!("projects/{project}/users?per_page=100")))
            .header("PRIVATE-TOKEN", token)
            .send()
            .await
            .map_err(network)?;
        let rows: Vec<User> = ensure(response).await?.json().await.map_err(network)?;
        Ok(logins
            .iter()
            .filter_map(|login| {
                rows.iter()
                    .find(|user| &user.username == login)
                    .map(|user| user.id)
            })
            .collect())
    }
    async fn milestone_id(
        &self,
        token: &str,
        project: &str,
        title: &str,
    ) -> Result<u64, ForgeError> {
        let response = reqwest::Client::new()
            .get(self.api(&format!("projects/{project}/milestones?state=active&per_page=100")))
            .header("PRIVATE-TOKEN", token)
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
            request_reviewers: true,
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
            create_change_request: true,
            edit_title: true,
            edit_description: true,
            labels: true,
            assignees: true,
            milestone: true,
            // GitLab marks drafts through the "Draft: " title prefix, which prtop manages.
            draft_transition: true,
            close: true,
            reopen: true,
            merge: true,
            merge_commit: true,
            squash_merge: true,
            rebase_merge: true,
            auto_merge: true,
            delete_source_branch: true,
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
    async fn get_change_request(&self, id: &ChangeRequestId) -> Result<ChangeRequest, ForgeError> {
        let token = self.credential().await?;
        self.get_mr(&token, id).await
    }
    async fn get_repository(&self, repository: &str) -> Result<RepositoryInfo, ForgeError> {
        let token = self.credential().await?;
        let encoded =
            url::form_urlencoded::byte_serialize(repository.as_bytes()).collect::<String>();
        let response = reqwest::Client::new()
            .get(self.api(&format!("projects/{encoded}")))
            .header("PRIVATE-TOKEN", token)
            .send()
            .await
            .map_err(network)?;
        let row: ProjectRow = ensure(response).await?.json().await.map_err(network)?;
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
        let encoded =
            url::form_urlencoded::byte_serialize(repository.as_bytes()).collect::<String>();
        let mut body = serde_json::json!({
            "source_branch": input.source_branch,
            "target_branch": input.target_branch,
            "title": if input.draft && !input.title.starts_with(DRAFT_PREFIX) {
                format!("{DRAFT_PREFIX}{}", input.title)
            } else {
                input.title.clone()
            },
            "description": input.body,
        });
        if !input.reviewers.is_empty() {
            let ids = self
                .user_ids(&token, &encoded, &input.reviewers)
                .await
                .unwrap_or_default();
            body["reviewer_ids"] = serde_json::json!(ids);
        }
        if !input.assignees.is_empty() {
            let ids = self
                .user_ids(&token, &encoded, &input.assignees)
                .await
                .unwrap_or_default();
            body["assignee_ids"] = serde_json::json!(ids);
        }
        if !input.labels.is_empty() {
            body["labels"] = serde_json::json!(input.labels.join(","));
        }
        if let Some(milestone) = &input.milestone {
            if let Ok(id) = self.milestone_id(&token, &encoded, milestone).await {
                body["milestone_id"] = serde_json::json!(id);
            }
        }
        let response = reqwest::Client::new()
            .post(self.api(&format!("projects/{encoded}/merge_requests")))
            .header("PRIVATE-TOKEN", &token)
            .json(&body)
            .send()
            .await
            .map_err(network)?;
        let row: Row = ensure(response).await?.json().await.map_err(network)?;
        Ok(normalize(&self.name, repository, row))
    }
    async fn update_change_request(
        &self,
        id: &ChangeRequestId,
        patch: &RequestPatch,
    ) -> Result<ChangeRequest, ForgeError> {
        let token = self.credential().await?;
        let mut body = serde_json::Map::new();
        if let Some(draft) = patch.draft {
            // GitLab marks drafts through the "Draft: " title prefix.
            let current = self.get_mr(token.as_str(), id).await?;
            let title = if draft {
                if current.title.starts_with(DRAFT_PREFIX) {
                    current.title.clone()
                } else {
                    format!("{DRAFT_PREFIX}{}", current.title)
                }
            } else {
                current
                    .title
                    .strip_prefix(DRAFT_PREFIX)
                    .unwrap_or(&current.title)
                    .to_owned()
            };
            body.insert("title".into(), serde_json::json!(title));
        } else if let Some(title) = &patch.title {
            body.insert("title".into(), serde_json::json!(title));
        }
        if let Some(body_text) = &patch.body {
            body.insert("description".into(), serde_json::json!(body_text));
        }
        if let Some(state) = patch.state {
            body.insert(
                "state_event".into(),
                serde_json::json!(match state {
                    RequestState::Open => "reopen",
                    RequestState::Closed | RequestState::Merged => "close",
                }),
            );
        }
        if let Some(labels) = &patch.labels {
            body.insert("labels".into(), serde_json::json!(labels.join(",")));
        }
        if let Some(assignees) = &patch.assignees {
            let ids = self
                .user_ids(&token, &Self::project(id), assignees)
                .await
                .unwrap_or_default();
            body.insert("assignee_ids".into(), serde_json::json!(ids));
        }
        if let Some(milestone) = &patch.milestone {
            let id = match milestone {
                Some(title) => self
                    .milestone_id(&token, &Self::project(id), title)
                    .await
                    .ok(),
                None => None,
            };
            body.insert("milestone_id".into(), serde_json::json!(id));
        }
        if !body.is_empty() {
            let response = reqwest::Client::new()
                .put(self.api(&format!(
                    "projects/{}/merge_requests/{}",
                    Self::project(id),
                    id.number
                )))
                .header("PRIVATE-TOKEN", &token)
                .json(&serde_json::json!(body))
                .send()
                .await
                .map_err(network)?;
            ensure(response).await?;
        }
        self.get_mr(&token, id).await
    }
    async fn list_labels(&self, repository: &str) -> Result<Vec<Label>, ForgeError> {
        let token = self.credential().await?;
        let encoded =
            url::form_urlencoded::byte_serialize(repository.as_bytes()).collect::<String>();
        let response = reqwest::Client::new()
            .get(self.api(&format!("projects/{encoded}/labels?per_page=100")))
            .header("PRIVATE-TOKEN", token)
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
        let patch = RequestPatch {
            labels: Some(names.to_vec()),
            ..RequestPatch::default()
        };
        let updated = self.update_change_request(id, &patch).await?;
        Ok(updated.labels)
    }
    async fn search_assignees(
        &self,
        _repository: &str,
        query: &str,
    ) -> Result<Vec<Person>, ForgeError> {
        let token = self.credential().await?;
        let response = reqwest::Client::new()
            .get(self.api(&format!(
                "users?search={}&per_page=50",
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
                name: user.name,
                id: Some(user.id),
            })
            .collect())
    }
    async fn set_assignees(
        &self,
        id: &ChangeRequestId,
        logins: &[String],
    ) -> Result<Vec<Person>, ForgeError> {
        let patch = RequestPatch {
            assignees: Some(logins.to_vec()),
            ..RequestPatch::default()
        };
        let updated = self.update_change_request(id, &patch).await?;
        Ok(updated.assignees)
    }
    async fn list_milestones(&self, repository: &str) -> Result<Vec<Milestone>, ForgeError> {
        let token = self.credential().await?;
        let encoded =
            url::form_urlencoded::byte_serialize(repository.as_bytes()).collect::<String>();
        let response = reqwest::Client::new()
            .get(self.api(&format!(
                "projects/{encoded}/milestones?state=active&per_page=100"
            )))
            .header("PRIVATE-TOKEN", token)
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
        let patch = RequestPatch {
            milestone: Some(milestone.map(str::to_owned)),
            ..RequestPatch::default()
        };
        let updated = self.update_change_request(id, &patch).await?;
        Ok(updated.milestone)
    }
    /// GitLab auto-merge is "merge when pipeline succeeds" on the merge endpoint; disabling
    /// uses its dedicated cancel endpoint.
    async fn set_auto_merge(
        &self,
        id: &ChangeRequestId,
        enable: bool,
        strategy: MergeStrategy,
    ) -> Result<bool, ForgeError> {
        let token = self.credential().await?;
        let path = if enable {
            format!("projects/{}/merge_requests/{}/merge", Self::project(id), id.number)
        } else {
            format!(
                "projects/{}/merge_requests/{}/cancel_merge_when_pipeline_succeeds",
                Self::project(id),
                id.number
            )
        };
        let mut request = reqwest::Client::new()
            .put(self.api(&path))
            .header("PRIVATE-TOKEN", &token);
        if enable {
            request = request.json(&serde_json::json!({
                "merge_when_pipeline_succeeds": true,
                "merge_method": strategy.api_name(),
            }));
        }
        let response = request.send().await.map_err(network)?;
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
                "projects/{}/merge_requests/{}/merge",
                Self::project(id),
                id.number
            )))
            .header("PRIVATE-TOKEN", &token)
            .json(&serde_json::json!({"merge_method": strategy.api_name()}))
            .send()
            .await
            .map_err(network)?;
        let status = response.status().as_u16();
        if status == 405 || status == 406 {
            return Err(ForgeError::Validation(
                "GitLab rejected the merge (not mergeable or head changed)".into(),
            ));
        }
        let row: Row = ensure(response).await?.json().await.map_err(network)?;
        Ok(MergeOutcome {
            sha: row.merge_commit_sha,
            message: None,
        })
    }
    async fn delete_branch(&self, repository: &str, branch: &str) -> Result<(), ForgeError> {
        let token = self.credential().await?;
        let encoded =
            url::form_urlencoded::byte_serialize(repository.as_bytes()).collect::<String>();
        let encoded_branch =
            url::form_urlencoded::byte_serialize(branch.as_bytes()).collect::<String>();
        let response = reqwest::Client::new()
            .delete(self.api(&format!(
                "projects/{encoded}/repository/branches/{encoded_branch}"
            )))
            .header("PRIVATE-TOKEN", token)
            .send()
            .await
            .map_err(network)?;
        ensure(response).await.map(|_| ())
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
                "users?search={}&per_page=50",
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
                name: user.name,
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
        let reviewer_id = reviewer.id.ok_or_else(|| {
            ForgeError::Validation("GitLab reviewers must be selected from search".into())
        })?;
        let current = self.get_mr(token.as_str(), id).await?;
        let mut ids: Vec<u64> = current.reviewers.iter().filter_map(|r| r.person.id).collect();
        if !ids.contains(&reviewer_id) {
            ids.push(reviewer_id);
        }
        let response = reqwest::Client::new()
            .put(self.api(&format!(
                "projects/{}/merge_requests/{}",
                Self::project(id),
                id.number
            )))
            .header("PRIVATE-TOKEN", token)
            .json(&serde_json::json!({"reviewer_ids": ids}))
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
        let current = self.get_mr(token.as_str(), id).await?;
        let ids: Vec<u64> = current
            .reviewers
            .iter()
            .filter(|r| r.person.login != reviewer)
            .filter_map(|r| r.person.id)
            .collect();
        let response = reqwest::Client::new()
            .put(self.api(&format!(
                "projects/{}/merge_requests/{}",
                Self::project(id),
                id.number
            )))
            .header("PRIVATE-TOKEN", token)
            .json(&serde_json::json!({"reviewer_ids": ids}))
            .send()
            .await
            .map_err(network)?;
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
    #[serde(default)]
    description: Option<String>,
    author: User,
    source_branch: String,
    target_branch: String,
    #[serde(default)]
    draft: bool,
    updated_at: DateTime<Utc>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    merged_at: Option<DateTime<Utc>>,
    #[serde(default)]
    web_url: Option<String>,
    #[serde(default)]
    has_conflicts: Option<bool>,
    #[serde(default)]
    detailed_merge_status: Option<String>,
    #[serde(default)]
    merge_when_pipeline_succeeds: bool,
    #[serde(default)]
    labels: Vec<LabelRow>,
    #[serde(default)]
    assignees: Vec<User>,
    #[serde(default)]
    milestone: Option<MilestoneRow>,
    #[serde(default)]
    reviewers: Vec<User>,
    #[serde(default)]
    sha: Option<String>,
    #[serde(default)]
    merge_commit_sha: Option<String>,
}
#[derive(Deserialize)]
struct User {
    username: String,
    #[serde(default)]
    id: u64,
    #[serde(default)]
    name: Option<String>,
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
#[derive(Deserialize)]
struct LabelRow {
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
struct ProjectRow {
    #[serde(default)]
    default_branch: Option<String>,
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
                name: self.author.name.clone(),
                id: Some(self.author.id),
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
fn normalize(forge: &str, repo: &str, row: Row) -> ChangeRequest {
    let state = request_state(&row);
    let mut request = normalized_request(
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
    );
    request.state = state;
    request.body = row.description.filter(|body| !body.is_empty());
    request.labels = row
        .labels
        .into_iter()
        .map(|label| Label {
            name: label.name,
            color: label.color,
        })
        .collect();
    request.assignees = row
        .assignees
        .iter()
        .map(|user| Person {
            login: user.username.clone(),
            name: user.name.clone(),
            id: Some(user.id),
        })
        .collect();
    request.milestone = row.milestone.as_ref().map(|milestone| milestone.title.clone());
    request.web_url = row.web_url;
    request.auto_merge = row.merge_when_pipeline_succeeds;
    request.mergeable_state = row.detailed_merge_status;
    request.head_sha = row.sha;
    request.merged_sha = row.merge_commit_sha;
    request.mergeability = match row.has_conflicts {
        Some(true) => crate::model::Mergeability::Conflicting,
        Some(false) => crate::model::Mergeability::Mergeable,
        None => crate::model::Mergeability::Unknown,
    };
    request.reviewers = row
        .reviewers
        .iter()
        .map(|user| Reviewer {
            person: Person {
                login: user.username.clone(),
                name: user.name.clone(),
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
    fn normalizes_gitlab_mr() {
        let row: Row=serde_json::from_str(r#"{"iid":43,"title":"Blocks","author":{"username":"jack"},"source_branch":"public","target_branch":"main","draft":true,"updated_at":"2026-08-29T12:00:00Z"}"#).unwrap();
        let item = normalize("work", "volt/volt.link", row);
        assert_eq!(item.id.display(item.kind), "!43");
        assert!(item.draft);
    }
    #[test]
    fn normalizes_note_write_response() {
        let row: Note = serde_json::from_str(r#"{"id":7,"body":"done","author":{"username":"jack","id":2},"created_at":"2026-08-29T12:00:00Z","updated_at":null,"web_url":null}"#).unwrap();
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

    #[test]
    fn normalizes_full_merge_request_metadata() {
        let row: Row = serde_json::from_str(r#"{"iid":52,"title":"Public blocks","description":"Body","author":{"username":"jack","id":1,"name":"Jack"},"source_branch":"public","target_branch":"main","draft":false,"state":"opened","updated_at":"2026-08-29T12:00:00Z","web_url":"https://gitlab.example.com/volt/volt.link/-/merge_requests/52","has_conflicts":false,"detailed_merge_status":"ci_still_running","merge_when_pipeline_succeeds":true,"labels":[{"name":"bug","color":"ff0000"}],"assignees":[{"username":"alice","id":2,"name":"Alice"}],"milestone":{"id":9,"title":"v1.2"},"reviewers":[{"username":"bob","id":3,"name":"Bob"}],"sha":"4e2f73a"}"#).unwrap();
        let item = normalize("work", "volt/volt.link", row);
        assert_eq!(item.state, RequestState::Open);
        assert_eq!(item.body.as_deref(), Some("Body"));
        assert_eq!(item.labels[0].name, "bug");
        assert_eq!(item.assignees[0].login, "alice");
        assert_eq!(item.milestone.as_deref(), Some("v1.2"));
        assert_eq!(
            item.web_url.as_deref(),
            Some("https://gitlab.example.com/volt/volt.link/-/merge_requests/52")
        );
        assert_eq!(item.mergeable_state.as_deref(), Some("ci_still_running"));
        assert!(item.auto_merge);
        assert_eq!(item.head_sha.as_deref(), Some("4e2f73a"));
        assert_eq!(item.reviewers[0].person.login, "bob");
    }

    #[test]
    fn closed_and_merged_mrs_map_to_their_state() {
        let closed: Row = serde_json::from_str(r#"{"iid":1,"title":"x","author":{"username":"jack"},"source_branch":"a","target_branch":"main","state":"closed","updated_at":"2026-08-29T12:00:00Z"}"#).unwrap();
        assert_eq!(normalize("work", "r", closed).state, RequestState::Closed);
        let merged: Row = serde_json::from_str(r#"{"iid":1,"title":"x","author":{"username":"jack"},"source_branch":"a","target_branch":"main","state":"closed","merged_at":"2026-08-29T13:00:00Z","merge_commit_sha":"abc","updated_at":"2026-08-29T12:00:00Z"}"#).unwrap();
        let request = normalize("work", "r", merged);
        assert_eq!(request.state, RequestState::Merged);
        assert_eq!(request.merged_sha.as_deref(), Some("abc"));
    }

    #[test]
    fn strategy_api_names_match_gitlab() {
        assert_eq!(MergeStrategy::MergeCommit.api_name(), "merge");
        assert_eq!(MergeStrategy::Squash.api_name(), "squash");
        assert_eq!(MergeStrategy::Rebase.api_name(), "rebase");
    }
}
