use crate::model::*;
use chrono::{Duration, Utc};

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
            author: Person { login: if index % 2 == 0 { "alice".into() } else { "bob".into() }, name: Some(if index % 2 == 0 { "Alice".into() } else { "Bob".into() }) },
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
                },
                state: ReviewState::Approved,
            },
            Reviewer {
                person: Person {
                    login: "bob".into(),
                    name: Some("Bob".into()),
                },
                state: if number == 184 {
                    ReviewState::Requested
                } else {
                    review
                },
            },
        ],
        pipelines: vec![pipeline(forge, repo, number + 200, &format!("workflow-{number}"), &format!("feature/{number}"), ci, updated_at, jobs)],
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
