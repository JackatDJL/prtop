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

/// Normalized lifecycle state. Open requests come from the dashboard listing; closed and
/// merged states are reached through provider writes and targeted reconciliation.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, Eq, PartialEq)]
pub enum RequestState {
    #[default]
    Open,
    Closed,
    Merged,
}
impl RequestState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::Merged => "merged",
        }
    }
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Open => "",
            Self::Closed => "✗",
            Self::Merged => "✓",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct Label {
    pub name: String,
    #[serde(default)]
    pub color: Option<String>,
}
impl Label {
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            color: None,
        }
    }
}

/// A provider-reported merge queue membership. Never synthesized by prtop.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct MergeQueue {
    #[serde(default)]
    pub position: Option<u32>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
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
    pub pipelines: Vec<Pipeline>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub state: RequestState,
    #[serde(default)]
    pub labels: Vec<Label>,
    #[serde(default)]
    pub assignees: Vec<Person>,
    #[serde(default)]
    pub milestone: Option<String>,
    #[serde(default)]
    pub web_url: Option<String>,
    #[serde(default)]
    pub auto_merge: bool,
    /// Provider's own merge-policy vocabulary (GitHub `mergeable_state`, GitLab
    /// `detailed_merge_status`). Policy and technical mergeability deliberately stay separate.
    #[serde(default)]
    pub mergeable_state: Option<String>,
    #[serde(default)]
    pub head_sha: Option<String>,
    #[serde(default)]
    pub merged_sha: Option<String>,
    /// Present only when a provider actually reports queue membership. prtop never invents one.
    #[serde(default)]
    pub merge_queue: Option<MergeQueue>,
}
#[derive(Deserialize)]
struct ChangeRequestWire {
    id: ChangeRequestId,
    kind: ChangeRequestKind,
    title: String,
    author: Person,
    source_branch: String,
    target_branch: String,
    draft: bool,
    mergeability: Mergeability,
    review: ReviewState,
    ci: CiState,
    updated_at: DateTime<Utc>,
    additions: u32,
    deletions: u32,
    comments: Vec<Comment>,
    reviewers: Vec<Reviewer>,
    #[serde(default)]
    pipelines: Vec<Pipeline>,
    #[serde(default)]
    pipeline: Option<LegacyPipeline>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    state: RequestState,
    #[serde(default)]
    labels: Vec<Label>,
    #[serde(default)]
    assignees: Vec<Person>,
    #[serde(default)]
    milestone: Option<String>,
    #[serde(default)]
    web_url: Option<String>,
    #[serde(default)]
    auto_merge: bool,
    #[serde(default)]
    mergeable_state: Option<String>,
    #[serde(default)]
    head_sha: Option<String>,
    #[serde(default)]
    merged_sha: Option<String>,
    #[serde(default)]
    merge_queue: Option<MergeQueue>,
}
impl<'de> Deserialize<'de> for ChangeRequest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = ChangeRequestWire::deserialize(deserializer)?;
        let pipelines = if wire.pipelines.is_empty() {
            wire.pipeline
                .map(|pipeline| legacy_pipeline(pipeline, &wire.id))
                .into_iter()
                .collect()
        } else {
            wire.pipelines
        };
        Ok(Self {
            id: wire.id,
            kind: wire.kind,
            title: wire.title,
            author: wire.author,
            source_branch: wire.source_branch,
            target_branch: wire.target_branch,
            draft: wire.draft,
            mergeability: wire.mergeability,
            review: wire.review,
            ci: wire.ci,
            updated_at: wire.updated_at,
            additions: wire.additions,
            deletions: wire.deletions,
            comments: wire.comments,
            reviewers: wire.reviewers,
            pipelines,
            body: wire.body,
            state: wire.state,
            labels: wire.labels,
            assignees: wire.assignees,
            milestone: wire.milestone,
            web_url: wire.web_url,
            auto_merge: wire.auto_merge,
            mergeable_state: wire.mergeable_state,
            head_sha: wire.head_sha,
            merged_sha: wire.merged_sha,
            merge_queue: wire.merge_queue,
        })
    }
}
#[derive(Deserialize)]
struct LegacyPipeline {
    number: u64,
    status: CiState,
    #[serde(default)]
    jobs: Vec<LegacyJob>,
}
#[derive(Deserialize)]
struct LegacyJob {
    name: String,
    status: CiState,
    duration_seconds: Option<u64>,
}
fn legacy_pipeline(legacy: LegacyPipeline, request: &ChangeRequestId) -> Pipeline {
    let id = PipelineId {
        forge: request.forge.clone(),
        repository: request.repository.clone(),
        value: legacy.number.to_string(),
    };
    Pipeline {
        id: id.clone(),
        name: format!("pipeline #{}", legacy.number),
        ref_name: String::new(),
        sha: String::new(),
        status: legacy.status.into(),
        created_at: Utc::now(),
        started_at: None,
        finished_at: None,
        stages: vec![],
        jobs: legacy
            .jobs
            .into_iter()
            .enumerate()
            .map(|(index, job)| Job {
                id: JobId {
                    pipeline: id.clone(),
                    value: (index + 1).to_string(),
                },
                name: job.name,
                stage: None,
                status: job.status.into(),
                started_at: None,
                finished_at: None,
                duration_seconds: job.duration_seconds,
                runner: None,
                attempt: 1,
                allow_failure: false,
                url: None,
                environment: None,
            })
            .collect(),
        url: None,
        environment: None,
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Person {
    pub login: String,
    pub name: Option<String>,
    /// Provider-native user id. GitLab needs it because its reviewer/assignee writes take ids.
    #[serde(default)]
    pub id: Option<u64>,
}
impl Person {
    pub fn named(login: impl Into<String>) -> Self {
        Self {
            login: login.into(),
            name: None,
            id: None,
        }
    }
    pub fn with_id(login: impl Into<String>, id: u64) -> Self {
        Self {
            login: login.into(),
            name: None,
            id: Some(id),
        }
    }
    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.login)
    }
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

/// Normalized merge strategies. Providers advertise which ones they accept; the UI never
/// renders an unsupported strategy.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub enum MergeStrategy {
    MergeCommit,
    Squash,
    Rebase,
}
impl MergeStrategy {
    pub fn label(self) -> &'static str {
        match self {
            Self::MergeCommit => "Merge commit",
            Self::Squash => "Squash",
            Self::Rebase => "Rebase",
        }
    }
    pub fn api_name(self) -> &'static str {
        match self {
            Self::MergeCommit => "merge",
            Self::Squash => "squash",
            Self::Rebase => "rebase",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, Eq, PartialEq)]
pub struct MergeOutcome {
    pub sha: Option<String>,
    pub message: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pipeline {
    pub id: PipelineId,    pub name: String,
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migrates_a_cached_singular_pipeline() {
        let request: ChangeRequest = serde_json::from_str(r#"{"id":{"forge":"github","repository":"jack/prtop","number":1},"kind":"PullRequest","title":"x","author":{"login":"jack","name":null},"source_branch":"x","target_branch":"main","draft":false,"mergeability":"Mergeable","review":"None","ci":"Passed","updated_at":"2026-08-29T12:00:00Z","additions":0,"deletions":0,"comments":[],"reviewers":[],"pipeline":{"number":9,"status":"Passed","jobs":[{"name":"test","status":"Passed","duration_seconds":3}]}}"#).unwrap();
        assert_eq!(request.pipelines.len(), 1);
        assert_eq!(request.pipelines[0].jobs[0].name, "test");
    }
}
impl From<CiState> for PipelineStatus {
    fn from(value: CiState) -> Self {
        match value {
            CiState::Passed => Self::Success,
            CiState::Failed => Self::Failed,
            CiState::Running => Self::Running,
            CiState::Pending => Self::Pending,
            CiState::None => Self::Unknown,
        }
    }
}
