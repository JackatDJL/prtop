//! Local Git jobs. Commands inherit the user's Git configuration and never run on the UI task.

use std::{path::Path, time::Duration};
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
    let task = Command::new("git").args(args).current_dir(path).output();
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
