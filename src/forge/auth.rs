use super::ForgeError;
use std::time::Duration;
use tokio::{process::Command, time::timeout};

/// Resolves a token without ever starting an interactive login or logging its value.
pub async fn resolve(
    environment: &[&str],
    command: &str,
    args: &[&str],
    hint: &str,
) -> Result<String, ForgeError> {
    for name in environment {
        if let Ok(value) = std::env::var(name)
            && !value.trim().is_empty()
        {
            return Ok(value);
        }
    }
    let output = timeout(
        Duration::from_secs(2),
        Command::new(command).args(args).output(),
    )
    .await
    .map_err(|_| ForgeError::AuthenticationRequired(hint.into()))?
    .map_err(|_| ForgeError::AuthenticationRequired(hint.into()))?;
    if output.status.success() {
        let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if !value.is_empty() {
            return Ok(value);
        }
    }
    Err(ForgeError::AuthenticationRequired(hint.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn missing_command_never_becomes_a_provider_error() {
        let result = resolve(
            &["PRTOP_TEST_EMPTY"],
            "definitely-not-prtop",
            &[],
            "run auth login",
        )
        .await;
        assert!(matches!(result, Err(ForgeError::AuthenticationRequired(_))));
    }
}
