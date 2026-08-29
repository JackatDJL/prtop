use crate::model::ChangeRequest;
use anyhow::{Context, Result};
use directories::ProjectDirs;
use std::{fs, path::PathBuf};

pub fn load() -> Result<Vec<ChangeRequest>> {
    let path = path()?;
    if !path.exists() {
        return Ok(vec![]);
    }
    Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
}
pub fn store(changes: &[ChangeRequest]) -> Result<()> {
    let path = path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec(changes)?).context("writing cache")
}
fn path() -> Result<PathBuf> {
    ProjectDirs::from("org", "prtop", "prtop")
        .map(|d| d.cache_dir().join("change-requests.json"))
        .context("no platform cache directory")
}
