mod app;
mod cache;
mod config;
mod forge;
mod git;
mod model;
mod scope;
mod ssh;
mod ui;

use std::{io, time::Duration};

use anyhow::Result;
use app::{App, AppEvent, Scope};
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::mpsc;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// Use deterministic fixture data and never contact a forge.
    #[arg(long)]
    demo: bool,
    /// Show every configured project even when run inside a Git repository.
    #[arg(long)]
    global: bool,
    /// Limit the initial view to a configured project, forge, repository, or PR URL.
    scope: Option<String>,
}

struct TerminalGuard;
impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
        Ok(Self)
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "prtop=info".into()))
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let mut config = config::Config::load_or_create()?;
    let startup = scope::resolve(
        &std::env::current_dir()?,
        cli.global,
        cli.demo,
        cli.scope.as_deref(),
    )
    .await;
    let mut startup_notice = None;
    let requested_scope = match &startup {
        scope::StartupScope::Project {
            host,
            repository,
            path,
        } => {
            if host == "github.com" {
                register_github_project(&mut config, repository, path);
                Some(Scope::Project {
                    host: host.clone(),
                    repository: repository.clone(),
                })
            } else if has_configured_project(&config, host, repository) {
                Some(Scope::Project {
                    host: host.clone(),
                    repository: repository.clone(),
                })
            } else {
                startup_notice = Some(format!(
                    "Repository detected: {host}/{repository}. No configured project matches this repository."
                ));
                None
            }
        }
        _ => cli.scope.clone().map(Scope::Exact),
    };
    let (events, mut receiver) = mpsc::unbounded_channel();
    let mut app = App::new(config, cli.demo, requested_scope, events.clone()).await?;
    if let scope::StartupScope::UnknownRepository { remote, .. } = startup {
        startup_notice = Some(format!(
            "Repository detected, but its remote is not recognized: {remote}"
        ));
    }
    app.toast = startup_notice;
    app.request_refresh();

    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    let result = run(&mut terminal, &mut app, &mut receiver).await;
    terminal.show_cursor()?;
    result
}

fn has_configured_project(config: &config::Config, host: &str, repository: &str) -> bool {
    config.projects.iter().any(|project| {
        project.repo == repository
            && config
                .forges
                .iter()
                .any(|forge| forge.name == project.forge && forge.host == host)
    })
}

fn register_github_project(config: &mut config::Config, repository: &str, path: &str) {
    let forge_name = config
        .forges
        .iter()
        .find(|forge| forge.host == "github.com" && matches!(forge.kind, config::ForgeKind::Github))
        .map(|forge| forge.name.clone())
        .unwrap_or_else(|| {
            let base = "github";
            let mut candidate = base.to_owned();
            let mut suffix = 2;
            while config.forges.iter().any(|forge| forge.name == candidate) {
                candidate = format!("{base}-{suffix}");
                suffix += 1;
            }
            config.forges.push(config::ForgeConfig {
                name: candidate.clone(),
                kind: config::ForgeKind::Github,
                host: "github.com".into(),
            });
            candidate
        });
    if !config
        .projects
        .iter()
        .any(|project| project.forge == forge_name && project.repo == repository)
    {
        config.projects.push(config::ProjectConfig {
            name: repository.into(),
            forge: forge_name,
            repo: repository.into(),
            path: Some(path.into()),
            host: None,
        });
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    #[test]
    fn registers_a_project_against_an_existing_github_forge() {
        let mut config = config::Config {
            forges: vec![config::ForgeConfig {
                name: "personal".into(),
                kind: config::ForgeKind::Github,
                host: "github.com".into(),
            }],
            ..config::Config::default()
        };

        register_github_project(&mut config, "jack/prtop", "/work/prtop");

        assert_eq!(config.forges.len(), 1);
        assert!(config.projects.iter().any(|project| {
            project.forge == "personal"
                && project.repo == "jack/prtop"
                && project.path.as_deref() == Some("/work/prtop")
        }));
    }

    #[test]
    fn github_registration_avoids_an_existing_forge_name() {
        let mut config = config::Config {
            forges: vec![config::ForgeConfig {
                name: "github".into(),
                kind: config::ForgeKind::Gitlab,
                host: "gitlab.example.test".into(),
            }],
            ..config::Config::default()
        };

        register_github_project(&mut config, "jack/prtop", "/work/prtop");

        assert!(config.forges.iter().any(|forge| forge.name == "github-2"));
        assert!(
            config
                .projects
                .iter()
                .any(|project| project.forge == "github-2")
        );
    }
}

async fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    receiver: &mut mpsc::UnboundedReceiver<AppEvent>,
) -> Result<()> {
    let mut tick = tokio::time::interval(Duration::from_millis(150));
    loop {
        terminal.draw(|frame| ui::draw(frame, app))?;
        tokio::select! {
            _ = tick.tick() => {
                while event::poll(Duration::ZERO)? {
                    match event::read()? {
                        Event::Key(key) if key.kind == KeyEventKind::Press => { if app.handle_key_event(key) { return Ok(()); } }
                        Event::Mouse(mouse) => app.handle_mouse(mouse),
                        _ => {}
                    }
                }
            }
            Some(message) = receiver.recv() => match message {
                AppEvent::Refresh(result) => app.apply_refresh(result),
                AppEvent::CommentWrite { temporary_id, result } => app.apply_comment_write(temporary_id, result),
                AppEvent::ReviewWrite { request, state, result } => app.apply_review_write(request, state, result),
                AppEvent::LogLoaded { job, chunk } => app.apply_log_chunk(job, chunk),
                AppEvent::PipelinesLoaded { request, pipelines } => app.apply_pipelines(request, pipelines),
                AppEvent::PipelineLoaded { id, pipeline } => app.apply_pipeline(id, *pipeline),
            }
        }
    }
}
