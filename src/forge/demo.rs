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
                Job {
                    name: "lint".into(),
                    status: CiState::Passed,
                    duration_seconds: Some(38),
                },
                Job {
                    name: "test".into(),
                    status: CiState::Passed,
                    duration_seconds: Some(82),
                },
                Job {
                    name: "android".into(),
                    status: CiState::Running,
                    duration_seconds: None,
                },
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
                Job {
                    name: "unit".into(),
                    status: CiState::Passed,
                    duration_seconds: Some(41),
                },
                Job {
                    name: "integration".into(),
                    status: CiState::Running,
                    duration_seconds: None,
                },
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
            vec![Job {
                name: "test_transfer_mobile".into(),
                status: CiState::Failed,
                duration_seconds: Some(133),
            }],
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

fn request(
    forge: &str,
    repo: &str,
    number: u64,
    kind: ChangeRequestKind,
    title: &str,
    ci: CiState,
    review: ReviewState,
    updated_at: chrono::DateTime<Utc>,
    jobs: Vec<Job>,
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
        comments: vec![
            Comment {
                author: Person {
                    login: "alice".into(),
                    name: Some("Alice".into()),
                },
                body: "Looks good. The reader handles the old fixture now.".into(),
                created_at: updated_at - Duration::minutes(8),
                resolved: None,
            },
            Comment {
                author: Person {
                    login: "bob".into(),
                    name: Some("Bob".into()),
                },
                body: "Could this keep the failure context?".into(),
                created_at: updated_at - Duration::minutes(4),
                resolved: Some(false),
            },
        ],
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
        pipeline: Some(Pipeline {
            number: number + 200,
            status: ci,
            jobs,
        }),
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
    }
}
