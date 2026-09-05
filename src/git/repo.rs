//! Local repository context for the create workflow. Everything here runs through the git
//! executor, so the user's credentials, SSH config, signing setup, and hooks stay theirs.

use crate::git::{self, GitResult};
use crate::scope::parse_remote;
use std::path::{Path, PathBuf};
use std::time::Duration;

const FAST: Duration = Duration::from_secs(5);
const SLOW: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RepoContext {
    pub root: Option<PathBuf>,
    pub remote: String,
    pub host: Option<String>,
    pub repository: Option<String>,
    pub default_branch: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BranchState {
    pub branch: String,
    pub upstream: Option<String>,
    pub remote_branch_exists: Option<bool>,
    pub ahead: Option<usize>,
    pub behind: Option<usize>,
    pub dirty: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PushError {
    TimedOut,
    Failed { stderr: String },
}

/// `git push -u <remote> <branch>`. Force variants are deliberately unconstructible here;
/// force pushes belong to the stacked-branch milestone with explicit confirmation.
pub fn push_args(remote: &str, branch: &str) -> Vec<String> {
    vec!["push".into(), "--set-upstream".into(), remote.into(), branch.into()]
}

pub fn repo_root(path: &Path) -> Option<PathBuf> {
    let GitResult::Completed { stdout, .. } =
        git::run(path, &["rev-parse", "--show-toplevel"], FAST)
    else {
        return None;
    };
    let root = stdout.trim();
    (!root.is_empty()).then(|| PathBuf::from(root))
}

pub fn current_branch(root: &Path) -> Option<String> {
    let GitResult::Completed { stdout, .. } =
        git::run(root, &["rev-parse", "--abbrev-ref", "HEAD"], FAST)
    else {
        return None;
    };
    let branch = stdout.trim().to_owned();
    (!branch.is_empty() && branch != "HEAD").then_some(branch)
}

pub fn remote_url(root: &Path, remote: &str) -> Option<String> {
    let GitResult::Completed { stdout, .. } =
        git::run(root, &["remote", "get-url", remote], FAST)
    else {
        return None;
    };
    let url = stdout.trim().to_owned();
    (!url.is_empty()).then_some(url)
}

/// The provider default branch when git's remote HEAD ref is set.
pub fn default_branch(root: &Path, remote: &str) -> Option<String> {
    let GitResult::Completed { stdout, .. } = git::run(
        root,
        &[
            "symbolic-ref",
            &format!("refs/remotes/{remote}/HEAD"),
        ],
        FAST,
    )
    else {
        return None;
    };
    stdout
        .trim()
        .strip_prefix(&format!("refs/remotes/{remote}/"))
        .map(str::to_owned)
}

pub fn probe(path: &Path) -> Result<RepoContext, String> {
    let Some(root) = repo_root(path) else {
        return Err("not inside a git repository".into());
    };
    let mut context = RepoContext {
        root: Some(root),
        remote: "origin".into(),
        ..RepoContext::default()
    };
    if let Some(url) = remote_url(path, "origin")
        && let Some((host, repository)) = parse_remote(&url)
    {
        context.host = Some(host);
        context.repository = Some(repository);
    }
    if let Some(root) = &context.root {
        context.default_branch = default_branch(root, &context.remote);
    }
    Ok(context)
}

/// Commits ahead of / behind a base ref using remote-tracking refs. Stale refs are a known
/// limitation, surfaced to the user as "local refs" in the preflight summary.
pub fn branch_state(root: &Path, base: &str, remote: &str) -> BranchState {
    let branch = current_branch(root).unwrap_or_default();
    let mut state = BranchState {
        branch: branch.clone(),
        ..BranchState::default()
    };
    if branch.is_empty() {
        return state;
    }
    let GitResult::Completed { stdout, .. } = git::run(
        root,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
        FAST,
    ) else {
        return state;
    };
    let upstream = stdout.trim().to_owned();
    state.upstream = (!upstream.is_empty()).then_some(upstream);
    state.remote_branch_exists = match git::run(
        root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/remotes/{remote}/{branch}"),
        ],
        FAST,
    ) {
        GitResult::Completed { .. } => Some(true),
        GitResult::Failed { .. } => Some(false),
        GitResult::TimedOut => None,
    };
    let GitResult::Completed { stdout: status, .. } =
        git::run(root, &["status", "--porcelain"], FAST)
    else {
        return state;
    };
    state.dirty = !status.trim().is_empty();
    let base_ref = format!("{remote}/{base}");
    let GitResult::Completed { stdout: counts, .. } = git::run(
        root,
        &[
            "rev-list",
            "--left-right",
            "--count",
            &format!("{base_ref}...HEAD"),
        ],
        FAST,
    ) else {
        return state;
    };
    let mut numbers = counts.trim().split_whitespace().map(str::to_owned);
    if let (Some(behind), Some(ahead)) = (numbers.next(), numbers.next()) {
        state.behind = behind.parse().ok();
        state.ahead = ahead.parse().ok();
    }
    state
}

/// Whether the source branch exists on the remote. Uses `ls-remote` so a stale local
/// remote-tracking ref cannot claim a branch exists.
pub async fn remote_branch_exists(
    root: &Path,
    remote: &str,
    branch: &str,
) -> Result<bool, PushError> {
    match git::run(root, &["ls-remote", "--heads", remote, branch], SLOW).await {
        GitResult::TimedOut => Err(PushError::TimedOut),
        GitResult::Failed { stderr } => Err(PushError::Failed { stderr }),
        GitResult::Completed { stdout, .. } => Ok(!stdout.trim().is_empty()),
    }
}

pub async fn push(root: &Path, remote: &str, branch: &str) -> Result<(), PushError> {
    let args = push_args(remote, branch);
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    match git::run(root, &borrowed, SLOW).await {
        GitResult::Completed { .. } => Ok(()),
        GitResult::TimedOut => Err(PushError::TimedOut),
        GitResult::Failed { stderr } => Err(PushError::Failed { stderr }),
    }
}

pub async fn delete_local_branch(root: &Path, branch: &str) -> Result<(), PushError> {
    match git::run(root, &["branch", "-d", branch], FAST).await {
        GitResult::Completed { .. } => Ok(()),
        GitResult::TimedOut => Err(PushError::TimedOut),
        GitResult::Failed { stderr } => Err(PushError::Failed { stderr }),
    }
}

/// Local and remote-tracking branch names for the target picker, without duplicates or the
/// current checkout noise of `git branch -a`.
pub fn branches(root: &Path, remote: &str) -> Vec<String> {
    let GitResult::Completed { stdout, .. } = git::run(
        root,
        &[
            "for-each-ref",
            &format!("--format=%(refname:short)"),
            "refs/heads",
            &format!("refs/remotes/{remote}"),
        ],
        FAST,
    ) else {
        return vec![];
    };
    let mut names: Vec<String> = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.ends_with("/HEAD"))
        .map(|line| {
            line.strip_prefix(&format!("{remote}/"))
                .unwrap_or(line)
                .to_owned()
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Single-commit subjects between the base and HEAD, oldest first.
pub fn commit_subjects(root: &Path, base: &str, remote: &str, limit: usize) -> Vec<String> {
    let GitResult::Completed { stdout, .. } = git::run(
        root,
        &[
            "log",
            &format!("--format=%s"),
            &format!("{remote}/{base}..HEAD"),
            &format!("-{limit}"),
        ],
        FAST,
    ) else {
        return vec![];
    };
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

pub fn find_template(root: &Path, kind: crate::model::ChangeRequestKind) -> Option<String> {
    use crate::model::ChangeRequestKind::*;
    let candidates: Vec<&[&str]> = match kind {
        PullRequest => vec![
            &[".github", "PULL_REQUEST_TEMPLATE.md"],
            &[".github", "pull_request_template.md"],
            &[".github", "PULL_REQUEST_TEMPLATE"],
            &["PULL_REQUEST_TEMPLATE.md"],
            &["docs", "PULL_REQUEST_TEMPLATE.md"],
            &[".gitea", "PULL_REQUEST_TEMPLATE.md"],
        ],
        MergeRequest => vec![
            &[".gitlab", "merge_request_templates"],
            &[".gitea", "merge_request_templates"],
        ],
    };
    for parts in candidates {
        let path: PathBuf = parts.iter().collect();
        let full = root.join(path);
        if full.is_file() {
            return std::fs::read_to_string(&full).ok();
        }
        if full.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&full) {
                let mut names: Vec<PathBuf> = entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
                    .collect();
                names.sort();
                if let Some(first) = names.first() {
                    return std::fs::read_to_string(first).ok();
                }
            }
        }
    }
    None
}

/// A default title that is only a suggestion: single-commit subject first, then a readable
/// branch name. Deliberately no LLM or network dependency.
pub fn title_from(branch: &str, subjects: &[String], commits_ahead: usize) -> String {
    if commits_ahead == 1 && subjects.len() == 1 {
        return subjects[0].clone();
    }
    branch
        .rsplit('/')
        .next()
        .unwrap_or(branch)
        .replace(['-', '_'], " ")
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ChangeRequestKind;

    #[test]
    fn push_never_carries_force_flags() {
        let args = push_args("origin", "feature/reader");
        assert_eq!(args, vec!["push", "--set-upstream", "origin", "feature/reader"]);
        assert!(args.iter().all(|arg| !arg.contains("force")));
    }

    #[test]
    fn titles_prefer_a_single_commit_subject() {
        assert_eq!(
            title_from("feature/fix-droplet-reader", &["Fix droplet reader".into()], 1),
            "Fix droplet reader"
        );
    }

    #[test]
    fn titles_fall_back_to_a_readable_branch_name() {
        assert_eq!(
            title_from("feature/fix-droplet-reader", &[], 4),
            "Fix droplet reader"
        );
        assert_eq!(title_from("new-renderer", &[], 1), "New renderer");
    }

    #[test]
    fn title_uses_the_last_path_segment_only() {
        assert_eq!(title_from("jack/feature/new-reader", &[], 2), "New reader");
    }

    #[test]
    fn template_detection_reads_a_github_template() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".github")).unwrap();
        std::fs::write(
            dir.path().join(".github/pull_request_template.md"),
            "## Summary\n",
        )
        .unwrap();
        assert_eq!(
            find_template(dir.path(), ChangeRequestKind::PullRequest).as_deref(),
            Some("## Summary\n")
        );
    }

    #[test]
    fn template_detection_reads_the_first_gitlab_template() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".gitlab/merge_request_templates")).unwrap();
        std::fs::write(
            dir.path().join(".gitlab/merge_request_templates/Default.md"),
            "## What\n",
        )
        .unwrap();
        assert_eq!(
            find_template(dir.path(), ChangeRequestKind::MergeRequest).as_deref(),
            Some("## What\n")
        );
    }
}
