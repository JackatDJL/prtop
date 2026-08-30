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
    /// Pipelines are kept separately from the compact dashboard CI summary. A change request
    /// may legitimately have several workflow runs for the same head SHA.
    #[serde(default)]
    pub pipelines: Vec<Pipeline>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Person {
    pub login: String,
    pub name: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    pub author: Person,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
    pub can_edit: bool,
    pub can_delete: bool,
    pub url: Option<String>,
    pub resolved: Option<bool>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reviewer {
    pub person: Person,
    pub state: ReviewState,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub enum ReviewState {
    Pending,
    Approved,
    ChangesRequested,
    Requested,
    Commented,
    Dismissed,
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
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Hash)]
pub struct PipelineId {
    pub forge: String,
    pub repository: String,
    pub value: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Hash)]
pub struct JobId {
    pub pipeline: PipelineId,
    pub value: String,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub enum PipelineStatus {
    Queued,
    Pending,
    Running,
    Success,
    Failed,
    Cancelled,
    Skipped,
    Manual,
    TimedOut,
    Waiting,
    Unknown,
}
impl PipelineStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Success => "success",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Skipped => "skipped",
            Self::Manual => "manual",
            Self::TimedOut => "timed out",
            Self::Waiting => "waiting",
            Self::Unknown => "unknown",
        }
    }
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Success => "✓",
            Self::Failed | Self::TimedOut => "✗",
            Self::Running => "●",
            Self::Manual => "▶",
            Self::Queued | Self::Pending | Self::Waiting => "…",
            Self::Cancelled | Self::Skipped => "-",
            Self::Unknown => "?",
        }
    }
    #[allow(dead_code)]
    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Queued | Self::Pending | Self::Running | Self::Waiting
        )
    }
    #[allow(dead_code)]
    pub fn ci_state(self) -> CiState {
        match self {
            Self::Success => CiState::Passed,
            Self::Failed | Self::TimedOut => CiState::Failed,
            Self::Running => CiState::Running,
            Self::Queued | Self::Pending | Self::Waiting | Self::Manual => CiState::Pending,
            _ => CiState::None,
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub enum Mergeability {
    Mergeable,
    Conflicting,
    Blocked,
    Unknown,
}
impl Mergeability {
    pub fn label(self) -> &'static str {
        match self {
            Self::Mergeable => "mergeable",
            Self::Conflicting => "conflicting",
            Self::Blocked => "blocked",
            Self::Unknown => "unknown",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pipeline {
    pub id: PipelineId,
    pub name: String,
    pub ref_name: String,
    pub sha: String,
    pub status: PipelineStatus,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub stages: Vec<PipelineStage>,
    pub jobs: Vec<Job>,
    pub url: Option<String>,
    pub environment: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PipelineStage {
    pub name: String,
    pub status: PipelineStatus,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub id: JobId,
    pub name: String,
    pub stage: Option<String>,
    pub status: PipelineStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub duration_seconds: Option<u64>,
    pub runner: Option<String>,
    pub attempt: u32,
    pub allow_failure: bool,
    pub url: Option<String>,
    pub environment: Option<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LogChunk {
    pub text: String,
    pub complete: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct Artifact {
    pub name: String,
    pub url: Option<String>,
    pub expired: bool,
}

impl ReviewState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::ChangesRequested => "changes",
            Self::Requested => "requested",
            Self::Commented => "commented",
            Self::Dismissed => "dismissed",
            Self::Waiting => "waiting",
            Self::None => "-",
        }
    }
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Pending => "…",
            Self::Approved => "✓",
            Self::ChangesRequested => "✗",
            Self::Requested | Self::Waiting => "?",
            Self::Commented => "•",
            Self::Dismissed => "-",
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
