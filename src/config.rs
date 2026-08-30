use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Config {
    #[serde(default, rename = "forge")]
    pub forges: Vec<ForgeConfig>,
    #[serde(default, rename = "project")]
    pub projects: Vec<ProjectConfig>,
    #[serde(default, rename = "host")]
    pub hosts: Vec<HostConfig>,
    #[serde(default)]
    pub ci: CiConfig,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CiConfig {
    #[serde(default = "default_running_refresh")]
    pub running_refresh_seconds: u64,
    #[serde(default = "default_finished_refresh")]
    pub finished_refresh_seconds: u64,
    #[serde(default = "default_max_log_lines")]
    pub max_log_lines: usize,
    #[serde(default = "default_follow_logs")]
    pub follow_logs_by_default: bool,
}
impl Default for CiConfig {
    fn default() -> Self {
        Self {
            running_refresh_seconds: default_running_refresh(),
            finished_refresh_seconds: default_finished_refresh(),
            max_log_lines: default_max_log_lines(),
            follow_logs_by_default: default_follow_logs(),
        }
    }
}
fn default_running_refresh() -> u64 {
    8
}
fn default_finished_refresh() -> u64 {
    60
}
fn default_max_log_lines() -> usize {
    30_000
}
fn default_follow_logs() -> bool {
    true
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ForgeConfig {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: ForgeKind,
    pub host: String,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ForgeKind {
    Github,
    Gitlab,
    Forgejo,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProjectConfig {
    pub name: String,
    pub forge: String,
    pub repo: String,
    pub path: Option<String>,
    pub host: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HostConfig {
    pub name: String,
    pub hostname: String,
    pub user: Option<String>,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
}
fn default_timeout() -> u64 {
    8
}

impl Config {
    pub fn load_or_create() -> Result<Self> {
        let path = Self::path()?;
        if !path.exists() {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&path, SAMPLE)?;
            return Ok(Self::default());
        }
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }
    pub fn path() -> Result<PathBuf> {
        ProjectDirs::from("org", "prtop", "prtop")
            .map(|d| d.config_dir().join("config.toml"))
            .context("no platform configuration directory")
    }
}

const SAMPLE: &str = r#"# prtop configuration. Tokens belong in your environment or existing forge CLIs.
# [[forge]]
# name = "github"
# type = "github"
# host = "github.com"
#
# [[project]]
# name = "QuickDrop"
# forge = "github"
# repo = "jack/quickdrop"
# path = "~/dev/quickdrop"
# host = "desktop"
#
# [[host]]
# name = "desktop"
# hostname = "djl-dev"
# user = "jack"
# timeout_seconds = 8
#
# [ci]
# running_refresh_seconds = 8
# finished_refresh_seconds = 60
# max_log_lines = 30000
# follow_logs_by_default = true
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multi_forge_config() {
        let config: Config = toml::from_str(
            r#"
            [[forge]]
            name = "work"
            type = "gitlab"
            host = "gitlab.example.test"
            [[host]]
            name = "remote"
            hostname = "remote.test"
            [[project]]
            name = "API"
            forge = "work"
            repo = "team/api"
            host = "remote"
            "#,
        )
        .unwrap();
        assert_eq!(config.forges.len(), 1);
        assert!(matches!(config.forges[0].kind, ForgeKind::Gitlab));
        assert_eq!(config.hosts[0].timeout_seconds, 8);
    }
}
