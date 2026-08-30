use crate::git::{self, GitResult};
use std::{path::Path, time::Duration};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StartupScope {
    Global,
    Project {
        host: String,
        repository: String,
        path: String,
    },
    UnknownRepository {
        remote: String,
        path: String,
    },
}

pub fn parse_remote(remote: &str) -> Option<(String, String)> {
    let value = remote.trim().trim_end_matches('/').trim_end_matches(".git");
    let (host, repository) = if let Some(rest) = value.strip_prefix("git@") {
        rest.split_once(':')?
    } else if let Some(rest) = value.strip_prefix("ssh://git@") {
        rest.split_once('/')?
    } else {
        let rest = value.strip_prefix("https://")?;
        rest.split_once('/')?
    };
    (!host.is_empty() && repository.contains('/')).then(|| (host.to_owned(), repository.to_owned()))
}

pub async fn resolve(
    path: &Path,
    explicit_global: bool,
    demo: bool,
    explicit: Option<&str>,
) -> StartupScope {
    if demo || explicit_global {
        return StartupScope::Global;
    }
    let target = explicit.map(Path::new).unwrap_or(path);
    let GitResult::Completed { stdout: root, .. } = git::run(
        target,
        &["rev-parse", "--show-toplevel"],
        Duration::from_secs(2),
    )
    .await
    else {
        return StartupScope::Global;
    };
    let root = root.trim().to_owned();
    let GitResult::Completed { stdout: remote, .. } = git::run(
        Path::new(&root),
        &["remote", "get-url", "origin"],
        Duration::from_secs(2),
    )
    .await
    else {
        return StartupScope::UnknownRepository {
            remote: "origin is not configured".into(),
            path: root,
        };
    };
    match parse_remote(&remote) {
        Some((host, repository)) => StartupScope::Project {
            host,
            repository,
            path: root,
        },
        None => StartupScope::UnknownRepository {
            remote: remote.trim().into(),
            path: root,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_common_remotes() {
        assert_eq!(
            parse_remote("git@github.com:jack/prtop.git"),
            Some(("github.com".into(), "jack/prtop".into()))
        );
        assert_eq!(
            parse_remote("https://gitlab.example.com/volt/volt.link.git"),
            Some(("gitlab.example.com".into(), "volt/volt.link".into()))
        );
        assert_eq!(
            parse_remote("ssh://git@codeberg.org/jack/foo.git"),
            Some(("codeberg.org".into(), "jack/foo".into()))
        );
    }

    #[tokio::test]
    async fn resolves_a_temporary_github_repository() {
        let directory = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(directory.path())
                .status()
                .unwrap()
        };
        assert!(run(&["init"]).success());
        assert!(run(&["remote", "add", "origin", "git@github.com:example/demo.git"]).success());
        assert_eq!(
            resolve(directory.path(), false, false, None).await,
            StartupScope::Project {
                host: "github.com".into(),
                repository: "example/demo".into(),
                path: directory.path().to_string_lossy().into_owned()
            }
        );
    }
}
