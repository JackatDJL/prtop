use crate::forge::{ForgeCapabilities, ForgeError, ForgeProvider, NewChangeRequest};
use crate::model::*;
use chrono::{Duration, Utc};
use std::sync::Arc;

pub const DEMO_FORGES: [&str; 3] = ["github", "volt-gitlab", "codeberg"];

/// Per-forge capability matrix so the demo exercises capability gating: Codeberg simulates a
/// provider without squash support, Volt GitLab without rebase or draft toggles.
pub fn capabilities_for(forge: &str) -> ForgeCapabilities {
    let base = ForgeCapabilities {
        comments: true,
        reviews: true,
        approve: true,
        request_changes: true,
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
        draft_transition: true,
        close: true,
        reopen: true,
        merge: true,
        merge_commit: true,
        squash_merge: true,
        rebase_merge: true,
        auto_merge: true,
        delete_source_branch: true,
    };
    match forge {
        "volt-gitlab" => ForgeCapabilities {
            request_changes: false,
            rebase_merge: false,
            ..base
        },
        "codeberg" => ForgeCapabilities {
            squash_merge: false,
            auto_merge: false,
            ..base
        },
        _ => base,
    }
}

pub fn demo_providers() -> Vec<(String, Arc<dyn ForgeProvider>)> {
    DEMO_FORGES
        .iter()
        .map(|name| ((*name).to_owned(), Arc::new(DemoProvider::new(name)) as Arc<dyn ForgeProvider>))
        .collect()
}

pub struct DemoProvider {
    name: String,
}
impl DemoProvider {
    pub fn new(name: &str) -> Self {
        Self { name: name.into() }
    }
}
#[async_trait::async_trait]
impl ForgeProvider for DemoProvider {
    fn name(&self) -> &str {
        &self.name
    }
    fn capabilities(&self) -> ForgeCapabilities {
        capabilities_for(&self.name)
    }
    async fn list_change_requests(&self) -> Result<Vec<ChangeRequest>, ForgeError> {
        Ok(change_requests()
            .into_iter()
            .filter(|request| request.id.forge == self.name)
            .collect())
    }
    async fn get_change_request(&self, id: &ChangeRequestId) -> Result<ChangeRequest, ForgeError> {
        change_requests()
            .into_iter()
            .find(|request| request.id == *id)
            .ok_or(ForgeError::NotFound)
    }
    async fn get_repository(&self, _repository: &str) -> Result<crate::forge::RepositoryInfo, ForgeError> {
        Ok(crate::forge::RepositoryInfo {
            default_branch: Some("main".into()),
        })
    }
}

/// Simulates a provider create response without any network access.
pub fn created_request(
    forge: &str,
    repository: &str,
    kind: ChangeRequestKind,
    number: u64,
    input: &NewChangeRequest,
) -> ChangeRequest {
    let now = Utc::now();
    let prefix = match kind {
        ChangeRequestKind::PullRequest => "https://demo.invalid",
        ChangeRequestKind::MergeRequest => "https://demo.invalid",
    };
    ChangeRequest {
        id: ChangeRequestId {
            forge: forge.into(),
            repository: repository.into(),
            number,
        },
        kind,
        title: input.title.clone(),
        author: Person {
            login: "jack".into(),
            name: Some("Jack".into()),
            id: None,
        },
        source_branch: input.source_branch.clone(),
        target_branch: input.target_branch.clone(),
        draft: input.draft,
        mergeability: Mergeability::Mergeable,
        review: ReviewState::None,
        ci: CiState::None,
        updated_at: now,
        additions: 0,
        deletions: 0,
        comments: vec![],
        reviewers: input
            .reviewers
            .iter()
            .map(|login| Reviewer {
                person: Person::named(login.clone()),
                state: ReviewState::Requested,
            })
            .collect(),
        pipelines: vec![],
        body: (!input.body.is_empty()).then(|| input.body.clone()),
        state: RequestState::Open,
        labels: input.labels.iter().map(|name| Label::named(name.clone())).collect(),
        assignees: input.assignees.iter().map(|login| Person::named(login.clone())).collect(),
        milestone: input.milestone.clone(),
        web_url: Some(format!("{prefix}/{repository}/requests/{number}")),
        auto_merge: false,
        mergeable_state: Some("clean".into()),
        head_sha: None,
        merged_sha: None,
        merge_queue: None,
    }
}

/// Synthetic picker datasets so demo flows never touch a network.
pub fn picker_items(kind: crate::picker::PickerKind) -> Vec<crate::picker::PickerItem> {
    use crate::picker::{PickerItem, PickerKind};
    match kind {
        PickerKind::TargetBranch => vec![
            PickerItem::simple("main"),
            PickerItem::simple("develop"),
            PickerItem::simple("release/1.0"),
        ],
        PickerKind::Reviewer | PickerKind::Assignee => vec![
            PickerItem::simple("alice"),
            PickerItem::simple("bob"),
            PickerItem::simple("carol"),
            PickerItem::simple("dave"),
        ],
        PickerKind::Label => vec![
            PickerItem::simple("bug"),
            PickerItem::simple("mobile"),
            PickerItem::simple("enhancement"),
            PickerItem::simple("documentation"),
        ],
        PickerKind::Milestone => vec![
            PickerItem::simple("v1.2"),
            PickerItem::simple("v1.3"),
        ],
    }
}

pub fn change_requests() -> Vec<ChangeRequest> {
    let now = Utc::now();
    vec![
        request(
            "github",
            "quickdrop",
            184,
            ChangeRequestKind::PullRequest,
            "Fix droplet reader",
            CiState::Passed,
            ReviewState::Approved,
            now - Duration::minutes(4),
            vec![
                ("lint", "lint", PipelineStatus::Success, Some(38)),
                ("test", "test", PipelineStatus::Success, Some(82)),
                ("android", "build", PipelineStatus::Running, None),
            ],
        ),
        request(
            "volt-gitlab",
            "volt/volt.link",
            43,
            ChangeRequestKind::MergeRequest,
            "Public blocks",
            CiState::Running,
            ReviewState::Requested,
            now - Duration::minutes(12),
            vec![
                ("unit", "test", PipelineStatus::Success, Some(41)),
                ("integration", "test", PipelineStatus::Running, None),
                ("deploy-preview", "deploy", PipelineStatus::Manual, None),
            ],
        ),
        request(
            "codeberg",
            "jack/foo",
            12,
            ChangeRequestKind::PullRequest,
            "New renderer",
            CiState::Failed,
            ReviewState::Waiting,
            now - Duration::minutes(27),
            vec![(
                "test_transfer_mobile",
                "test",
                PipelineStatus::Failed,
                Some(133),
            )],
        ),
        request(
            "github",
            "quickdrop",
            181,
            ChangeRequestKind::PullRequest,
            "Rust decoder",
            CiState::Pending,
            ReviewState::None,
            now - Duration::hours(2),
            vec![],
        ),
    ]
}

#[allow(clippy::too_many_arguments)] // Fixture constructor mirrors the visible dashboard dimensions.
fn request(
    forge: &str,
    repo: &str,
    number: u64,
    kind: ChangeRequestKind,
    title: &str,
    ci: CiState,
    review: ReviewState,
    updated_at: chrono::DateTime<Utc>,
    jobs: Vec<(&str, &str, PipelineStatus, Option<u64>)>,
) -> ChangeRequest {
    ChangeRequest {
        id: ChangeRequestId {
            forge: forge.into(),
            repository: repo.into(),
            number,
        },
        kind,
        title: title.into(),
        author: Person {
            login: "jack".into(),
            name: Some("Jack".into()),
            id: None,
        },
        source_branch: format!("feature/{}", number),
        target_branch: "main".into(),
        draft: number == 181,
        mergeability: if number == 12 {
            Mergeability::Blocked
        } else {
            Mergeability::Mergeable
        },
        review,
        ci,
        updated_at,
        additions: 318,
        deletions: 84,
        comments: (0..42).map(|index| Comment {
            id: format!("{number}-{index}"),
            author: Person { login: if index % 2 == 0 { "alice".into() } else { "bob".into() }, name: Some(if index % 2 == 0 { "Alice".into() } else { "Bob".into() }), id: None },
            body: if index == 41 { "Looks good overall.\n\nOne concern about the error context, but the reader change itself is solid.".into() } else { format!("Discussion note {} for this change request.", index + 1) },
            created_at: updated_at - Duration::minutes(42 - index),
            updated_at: (index == 40).then_some(updated_at - Duration::minutes(1)),
            can_edit: index % 2 == 1,
            can_delete: index % 2 == 1,
            url: None,
            resolved: (index == 3).then_some(false),
        }).collect(),
        reviewers: vec![
            Reviewer {
                person: Person {
                    login: "alice".into(),
                    name: Some("Alice".into()),
                    id: None,
                },
                state: ReviewState::Approved,
            },
            Reviewer {
                person: Person {
                    login: "bob".into(),
                    name: Some("Bob".into()),
                    id: None,
                },
                state: if number == 184 {
                    ReviewState::Requested
                } else {
                    review
                },
            },
        ],
        pipelines: vec![pipeline(forge, repo, number + 200, &format!("workflow-{number}"), &format!("feature/{number}"), ci, updated_at, jobs)],
        body: Some(format!("Droplet reader fix for #{number}. See the failing transfer mobile job.")),
        state: RequestState::Open,
        labels: if number == 184 {
            vec![Label::named("bug"), Label::named("mobile")]
        } else {
            vec![Label::named("enhancement")]
        },
        assignees: vec![Person {
            login: "jack".into(),
            name: Some("Jack".into()),
            id: None,
        }],
        milestone: (number == 184).then(|| "v1.2".into()),
        web_url: match kind {
            ChangeRequestKind::PullRequest => {
                Some(format!("https://demo.invalid/{repo}/pulls/{number}"))
            }
            ChangeRequestKind::MergeRequest => {
                Some(format!("https://demo.invalid/{repo}/merge_requests/{number}"))
            }
        },
        auto_merge: false,
        mergeable_state: match number {
            184 => Some("clean".into()),
            12 => Some("blocked".into()),
            43 => Some("ci_still_running".into()),
            _ => Some("clean".into()),
        },
        head_sha: Some("4e2f73a".into()),
        merged_sha: None,
        merge_queue: (number == 43).then(|| MergeQueue {
            position: Some(3),
            name: Some("default".into()),
        }),
    }
}

#[allow(clippy::too_many_arguments)] // Fixture mirrors the user-visible pipeline identity.
fn pipeline(
    forge: &str,
    repo: &str,
    number: u64,
    name: &str,
    branch: &str,
    ci: CiState,
    created_at: chrono::DateTime<Utc>,
    jobs: Vec<(&str, &str, PipelineStatus, Option<u64>)>,
) -> Pipeline {
    let id = PipelineId {
        forge: forge.into(),
        repository: repo.into(),
        value: number.to_string(),
    };
    Pipeline {
        id: id.clone(),
        name: name.into(),
        ref_name: branch.into(),
        sha: "4e2f73a".into(),
        status: match ci {
            CiState::Passed => PipelineStatus::Success,
            CiState::Failed => PipelineStatus::Failed,
            CiState::Running => PipelineStatus::Running,
            CiState::Pending => PipelineStatus::Pending,
            CiState::None => PipelineStatus::Unknown,
        },
        created_at,
        started_at: Some(created_at),
        finished_at: None,
        stages: vec![
            PipelineStage {
                name: "lint".into(),
                status: PipelineStatus::Success,
            },
            PipelineStage {
                name: "test".into(),
                status: PipelineStatus::Running,
            },
            PipelineStage {
                name: "deploy".into(),
                status: PipelineStatus::Manual,
            },
        ],
        jobs: jobs
            .into_iter()
            .enumerate()
            .map(|(index, (name, stage, status, duration_seconds))| Job {
                id: JobId {
                    pipeline: id.clone(),
                    value: format!("{}-{}", number, index + 1),
                },
                name: name.into(),
                stage: Some(stage.into()),
                status,
                started_at: Some(created_at),
                finished_at: None,
                duration_seconds,
                runner: Some("linux-x64".into()),
                attempt: 1,
                allow_failure: false,
                url: None,
                environment: (stage == "deploy").then_some("preview".into()),
            })
            .collect(),
        url: None,
        environment: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_each_initial_provider_and_ci_state() {
        let requests = change_requests();
        assert!(requests.iter().any(|request| request.id.forge == "github"));
        assert!(
            requests
                .iter()
                .any(|request| request.kind == ChangeRequestKind::MergeRequest)
        );
        assert!(requests.iter().any(|request| request.ci == CiState::Failed));
        assert!(
            requests
                .iter()
                .any(|request| request.ci == CiState::Running)
        );
        assert_eq!(requests[0].comments.len(), 42);
        assert!(
            requests[0]
                .comments
                .iter()
                .any(|comment| comment.updated_at.is_some())
        );
    }
}
