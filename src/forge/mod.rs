pub mod demo;
pub mod forgejo;
pub mod github;
pub mod gitlab;

use crate::model::{
    ChangeRequest, ChangeRequestId, ChangeRequestKind, CiState, Comment, Mergeability, Person,
    Pipeline, ReviewState, Reviewer,
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
    #[error("operation is not implemented by this provider yet")]
    Unsupported,
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
    async fn get_pipeline(&self, _id: &ChangeRequestId) -> Result<Option<Pipeline>, ForgeError> {
        Err(ForgeError::Unsupported)
    }
    async fn submit_review(&self, _id: &ChangeRequestId) -> Result<(), ForgeError> {
        Err(ForgeError::Unsupported)
    }
    async fn create_comment(&self, _id: &ChangeRequestId, _body: &str) -> Result<(), ForgeError> {
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
        pipeline: None,
    }
}
