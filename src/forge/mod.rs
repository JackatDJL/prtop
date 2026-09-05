pub mod demo;
pub mod forgejo;
pub mod github;
pub mod gitlab;

use crate::model::{
    ChangeRequest, ChangeRequestId, ChangeRequestKind, CiState, Comment, JobId, Label, LogChunk,
    Mergeability, MergeOutcome, MergeStrategy, Person, Pipeline, PipelineId, RequestState,
    ReviewState, Reviewer,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use thiserror::Error;

#[allow(dead_code)] // The contract deliberately precedes the later write-operation milestones.
#[derive(Debug, Error)]
pub enum ForgeError {
    #[error("authentication required for {0}")]
    AuthenticationRequired(String),
    #[error("provider unavailable: {0}")]
    Unavailable(String),
    #[error("permission denied")]
    PermissionDenied,
    #[error("rate limited")]
    RateLimited { retry_after_seconds: Option<u64> },
    #[error("resource not found")]
    NotFound,
    #[error("validation failed: {0}")]
    Validation(String),
    #[error("conflict")]
    Conflict,
    #[error("operation is not implemented by this provider yet")]
    Unsupported,
    #[error("job cannot be retried")]
    JobNotRetryable,
    #[error("pipeline cannot be cancelled")]
    PipelineNotCancelable,
    #[error("logs are unavailable")]
    LogsUnavailable,
    #[error("pipeline has expired")]
    PipelineExpired,
    #[error("artifact has expired")]
    ArtifactExpired,
}

#[allow(dead_code)] // Constructed once app write dispatch routes through the provider registry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReviewAction {
    Approve,
    RequestChanges,
    Comment,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ForgeCapabilities {
    pub comments: bool,
    pub reviews: bool,
    pub approve: bool,
    pub request_changes: bool,
    pub request_reviewers: bool,
    pub edit_comments: bool,
    pub delete_comments: bool,
    pub ci_read: bool,
    pub ci_logs: bool,
    pub ci_retry_job: bool,
    pub ci_retry_pipeline: bool,
    pub ci_cancel_job: bool,
    pub ci_cancel_pipeline: bool,
    pub ci_play_manual: bool,
    pub ci_artifacts: bool,
    // Milestone 4: change-request lifecycle and metadata.
    pub create_change_request: bool,
    pub edit_title: bool,
    pub edit_description: bool,
    pub labels: bool,
    pub assignees: bool,
    pub milestone: bool,
    pub draft_transition: bool,
    pub close: bool,
    pub reopen: bool,
    pub merge: bool,
    pub merge_commit: bool,
    pub squash_merge: bool,
    pub rebase_merge: bool,
    pub auto_merge: bool,
    pub delete_source_branch: bool,
}

/// An input to change-request creation. Only fields the provider advertises are shown; the
/// rest stay empty and providers ignore what they do not support.
#[derive(Clone, Debug, Default)]
pub struct NewChangeRequest {
    pub title: String,
    pub body: String,
    pub source_branch: String,
    pub target_branch: String,
    pub draft: bool,
    pub reviewers: Vec<String>,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    pub milestone: Option<String>,
}

/// Metadata edits on an existing change request. Absent fields are left untouched.
#[derive(Clone, Debug, Default)]
pub struct RequestPatch {
    pub title: Option<String>,
    pub body: Option<String>,
    /// `true` marks the request draft, `false` marks it ready for review.
    pub draft: Option<bool>,
    /// `Closed` closes, `Open` reopens.
    pub state: Option<RequestState>,
    pub labels: Option<Vec<String>>,
    pub assignees: Option<Vec<String>>,
    pub milestone: Option<Option<String>>,
}

/// Provider-reported metadata about a repository, e.g. its default branch.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RepositoryInfo {
    pub default_branch: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Milestone {
    pub name: String,
}

#[allow(dead_code)] // Providers implement these methods as their milestone reaches the UI.
#[async_trait]
pub trait ForgeProvider: Send + Sync {
    fn name(&self) -> &str;
    fn capabilities(&self) -> ForgeCapabilities {
        ForgeCapabilities::default()
    }
    async fn list_change_requests(&self) -> Result<Vec<ChangeRequest>, ForgeError>;
    async fn get_change_request(&self, _id: &ChangeRequestId) -> Result<ChangeRequest, ForgeError> {
        Err(ForgeError::Unsupported)
    }
    async fn get_repository(&self, _repository: &str) -> Result<RepositoryInfo, ForgeError> {
        Err(ForgeError::Unsupported)
    }
    async fn create_change_request(
        &self,
        _input: &NewChangeRequest,
        _repository: &str,
    ) -> Result<ChangeRequest, ForgeError> {
        Err(ForgeError::Unsupported)
    }
    async fn update_change_request(
        &self,
        _id: &ChangeRequestId,
        _patch: &RequestPatch,
    ) -> Result<ChangeRequest, ForgeError> {
        Err(ForgeError::Unsupported)
    }
    async fn list_labels(
        &self,
        _repository: &str,
    ) -> Result<Vec<Label>, ForgeError> {
        Err(ForgeError::Unsupported)
    }
    async fn set_labels(
        &self,
        _id: &ChangeRequestId,
        _names: &[String],
    ) -> Result<Vec<Label>, ForgeError> {
        Err(ForgeError::Unsupported)
    }
    async fn search_assignees(
        &self,
        _repository: &str,
        _query: &str,
    ) -> Result<Vec<Person>, ForgeError> {
        Err(ForgeError::Unsupported)
    }
    async fn set_assignees(
        &self,
        _id: &ChangeRequestId,
        _logins: &[String],
    ) -> Result<Vec<Person>, ForgeError> {
        Err(ForgeError::Unsupported)
    }
    async fn list_milestones(
        &self,
        _repository: &str,
    ) -> Result<Vec<Milestone>, ForgeError> {
        Err(ForgeError::Unsupported)
    }
    async fn set_milestone(
        &self,
        _id: &ChangeRequestId,
        _milestone: Option<&str>,
    ) -> Result<Option<String>, ForgeError> {
        Err(ForgeError::Unsupported)
    }
    async fn set_auto_merge(
        &self,
        _id: &ChangeRequestId,
        _enable: bool,
        _strategy: MergeStrategy,
    ) -> Result<bool, ForgeError> {
        Err(ForgeError::Unsupported)
    }
    async fn merge_change_request(
        &self,
        _id: &ChangeRequestId,
        _strategy: MergeStrategy,
    ) -> Result<MergeOutcome, ForgeError> {
        Err(ForgeError::Unsupported)
    }
    async fn delete_branch(&self, _repository: &str, _branch: &str) -> Result<(), ForgeError> {
        Err(ForgeError::Unsupported)
    }
    async fn list_comments(
        &self,
        _id: &ChangeRequestId,
        _page: u32,
    ) -> Result<Vec<Comment>, ForgeError> {
        Err(ForgeError::Unsupported)
    }
    async fn list_reviews(&self, _id: &ChangeRequestId) -> Result<Vec<Reviewer>, ForgeError> {
        Err(ForgeError::Unsupported)
    }
    async fn list_pipelines(&self, _id: &ChangeRequestId) -> Result<Vec<Pipeline>, ForgeError> {
        Err(ForgeError::Unsupported)
    }
    async fn get_pipeline(&self, _id: &PipelineId) -> Result<Pipeline, ForgeError> {
        Err(ForgeError::Unsupported)
    }
    async fn get_job_log(&self, _id: &JobId, _offset: usize) -> Result<LogChunk, ForgeError> {
        Err(ForgeError::LogsUnavailable)
    }
    async fn retry_job(&self, _id: &JobId) -> Result<(), ForgeError> {
        Err(ForgeError::JobNotRetryable)
    }
    async fn retry_pipeline(&self, _id: &PipelineId) -> Result<(), ForgeError> {
        Err(ForgeError::Unsupported)
    }
    async fn cancel_job(&self, _id: &JobId) -> Result<(), ForgeError> {
        Err(ForgeError::PipelineNotCancelable)
    }
    async fn cancel_pipeline(&self, _id: &PipelineId) -> Result<(), ForgeError> {
        Err(ForgeError::PipelineNotCancelable)
    }
    async fn play_job(&self, _id: &JobId) -> Result<(), ForgeError> {
        Err(ForgeError::Unsupported)
    }
    async fn submit_review(&self, _id: &ChangeRequestId) -> Result<(), ForgeError> {
        Err(ForgeError::Unsupported)
    }
    async fn create_comment(&self, _id: &ChangeRequestId, _body: &str) -> Result<(), ForgeError> {
        Err(ForgeError::Unsupported)
    }
    async fn edit_comment(
        &self,
        _id: &ChangeRequestId,
        _comment_id: &str,
        _body: &str,
    ) -> Result<Comment, ForgeError> {
        Err(ForgeError::Unsupported)
    }
    async fn delete_comment(
        &self,
        _id: &ChangeRequestId,
        _comment_id: &str,
    ) -> Result<(), ForgeError> {
        Err(ForgeError::Unsupported)
    }
    async fn submit_review_action(
        &self,
        _id: &ChangeRequestId,
        _action: ReviewAction,
        _body: &str,
    ) -> Result<(), ForgeError> {
        Err(ForgeError::Unsupported)
    }
    async fn search_reviewers(
        &self,
        _id: &ChangeRequestId,
        _query: &str,
    ) -> Result<Vec<Person>, ForgeError> {
        Err(ForgeError::Unsupported)
    }
    async fn request_reviewer(
        &self,
        _id: &ChangeRequestId,
        _reviewer: &Person,
    ) -> Result<(), ForgeError> {
        Err(ForgeError::Unsupported)
    }
    async fn remove_reviewer(
        &self,
        _id: &ChangeRequestId,
        _reviewer: &str,
    ) -> Result<(), ForgeError> {
        Err(ForgeError::Unsupported)
    }
    async fn merge(&self, _id: &ChangeRequestId) -> Result<(), ForgeError> {
        Err(ForgeError::Unsupported)
    }
}

#[allow(clippy::too_many_arguments)] // Kept private while adapters share the normalized mapping.
pub(crate) fn normalized_request(
    forge: String,
    repository: String,
    number: u64,
    kind: ChangeRequestKind,
    title: String,
    author: String,
    source_branch: String,
    target_branch: String,
    draft: bool,
    updated_at: DateTime<Utc>,
) -> ChangeRequest {
    ChangeRequest {
        id: ChangeRequestId {
            forge,
            repository,
            number,
        },
        kind,
        title,
        author: Person {
            login: author,
            name: None,
            id: None,
        },
        source_branch,
        target_branch,
        draft,
        mergeability: Mergeability::Unknown,
        review: ReviewState::None,
        ci: CiState::None,
        updated_at,
        additions: 0,
        deletions: 0,
        comments: vec![],
        reviewers: vec![],
        pipelines: vec![],
        body: None,
        state: RequestState::Open,
        labels: vec![],
        assignees: vec![],
        milestone: None,
        web_url: None,
        auto_merge: false,
        mergeable_state: None,
        head_sha: None,
        merged_sha: None,
        merge_queue: None,
    }
}
pub mod auth;
