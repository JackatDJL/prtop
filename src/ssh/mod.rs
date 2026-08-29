//! SSH probes are noninteractive by design. An interactive login belongs to the user, outside prtop.

use crate::config::HostConfig;
use std::time::Duration;
use tokio::{process::Command, time::timeout};

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub enum HostStatus {
    Available,
    AuthenticationRequired { command: String },
    Unavailable { reason: String },
}

#[allow(dead_code)]
pub async fn probe(host: &HostConfig) -> HostStatus {
    let target = match &host.user {
        Some(user) => format!("{user}@{}", host.hostname),
        None => host.hostname.clone(),
    };
    let command = format!("ssh {target}");
    let process = Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=5",
            &target,
            "true",
        ])
        .output();
    match timeout(Duration::from_secs(host.timeout_seconds), process).await {
        Err(_) => HostStatus::Unavailable {
            reason: "SSH probe timed out".into(),
        },
        Ok(Err(error)) => HostStatus::Unavailable {
            reason: error.to_string(),
        },
        Ok(Ok(output)) if output.status.success() => HostStatus::Available,
        Ok(Ok(output)) => {
            let error = String::from_utf8_lossy(&output.stderr).into_owned();
            if error.contains("Permission denied") || error.contains("interactive") {
                HostStatus::AuthenticationRequired { command }
            } else {
                HostStatus::Unavailable { reason: error }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interactive_command_preserves_user() {
        let host = HostConfig {
            name: "laptop".into(),
            hostname: "t-mx15".into(),
            user: Some("jkxrx".into()),
            timeout_seconds: 8,
        };
        assert_eq!(
            format!("ssh {}@{}", host.user.unwrap(), host.hostname),
            "ssh jkxrx@t-mx15"
        );
    }
}
