//! Local Git jobs. Commands inherit the user's Git configuration and never run on the UI task.

pub mod repo;

use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{process::Command, time::timeout};

#[derive(Debug)]
#[allow(dead_code)]
pub enum GitResult {
    Completed { stdout: String, stderr: String },
    Failed { stderr: String },
    TimedOut,
}

#[allow(dead_code)]
pub async fn run(path: &Path, args: &[&str], limit: Duration) -> GitResult {
    let task = Command::new(trusted_executable())
        .args(args)
        .current_dir(path)
        .output();
    match timeout(limit, task).await {
        Err(_) => GitResult::TimedOut,
        Ok(Err(error)) => GitResult::Failed {
            stderr: error.to_string(),
        },
        Ok(Ok(output)) if output.status.success() => GitResult::Completed {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        },
        Ok(Ok(output)) => GitResult::Failed {
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        },
    }
}

#[cfg(unix)]
fn trusted_executable() -> PathBuf {
    PathBuf::from("/usr/bin/git")
}

#[cfg(windows)]
fn trusted_executable() -> PathBuf {
    let candidates = [
        PathBuf::from(r"C:\Program Files\Git\cmd\git.exe"),
        PathBuf::from(r"C:\Program Files (x86)\Git\cmd\git.exe"),
    ];
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files\Git\cmd\git.exe"))
}
