use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Hash)]
pub struct ChangeRequestId {
    pub forge: String,
    pub repository: String,
    pub number: u64,
}

impl ChangeRequestId {
    pub fn display(&self, kind: ChangeRequestKind) -> String {
        format!("{}{}", kind.number_prefix(), self.number)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub enum ChangeRequestKind {
    PullRequest,
    MergeRequest,
}
impl ChangeRequestKind {
    pub fn number_prefix(self) -> char {
        match self {
            Self::PullRequest => '#',
            Self::MergeRequest => '!',
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChangeRequest {
    pub id: ChangeRequestId,
    pub kind: ChangeRequestKind,
    pub title: String,
    pub author: Person,
    pub source_branch: String,
    pub target_branch: String,
    pub draft: bool,
    pub mergeability: Mergeability,
    pub review: ReviewState,
    pub ci: CiState,
    pub updated_at: DateTime<Utc>,
    pub additions: u32,
    pub deletions: u32,
    pub comments: Vec<Comment>,
    pub reviewers: Vec<Reviewer>,
    pub pipeline: Option<Pipeline>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Person {
    pub login: String,
    pub name: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Comment {
    pub author: Person,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub resolved: Option<bool>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reviewer {
    pub person: Person,
    pub state: ReviewState,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub enum ReviewState {
    Approved,
    ChangesRequested,
    Requested,
    Commented,
    Waiting,
    None,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub enum CiState {
    Passed,
    Failed,
    Running,
    Pending,
    None,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub enum Mergeability {
    Mergeable,
    Conflicting,
    Blocked,
    Unknown,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pipeline {
    pub number: u64,
    pub status: CiState,
    pub jobs: Vec<Job>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub name: String,
    pub status: CiState,
    pub duration_seconds: Option<u64>,
}

impl ReviewState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::ChangesRequested => "changes",
            Self::Requested => "requested",
            Self::Commented => "commented",
            Self::Waiting => "waiting",
            Self::None => "-",
        }
    }
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Approved => "✓",
            Self::ChangesRequested => "✗",
            Self::Requested | Self::Waiting => "?",
            Self::Commented => "•",
            Self::None => "·",
        }
    }
}
impl CiState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Running => "running",
            Self::Pending => "queued",
            Self::None => "-",
        }
    }
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Passed => "✓",
            Self::Failed => "✗",
            Self::Running => "●",
            Self::Pending => "…",
            Self::None => "·",
        }
    }
}
