mod app;
mod cache;
mod config;
mod create;
mod edit;
mod editor;
mod forge;
mod git;
mod merge;
mod model;
mod picker;
mod scope;
mod ssh;
mod ui;
mod write;

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
#[command(
    version,
    about,
    after_help = "Examples (Cargo):\n  cargo run\n  cargo run -- .\n  cargo run -- ~/dev/prtop\n  cargo run -- --global\n  cargo run -- --demo\n\nInstalled binary:\n  prtop\n  prtop .\n  prtop ~/dev/prtop\n  prtop --global\n  prtop --demo"
)]
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
    if let Some(notice) = startup_notice {
        app.set_toast(notice);
    }
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
    use clap::CommandFactory;

    #[test]
    fn cli_accepts_implicit_cwd_explicit_dot_path_global_and_demo_modes() {
        let implicit = Cli::try_parse_from(["prtop"]).unwrap();
        assert_eq!(implicit.scope, None);
        assert!(!implicit.global && !implicit.demo);

        let current = Cli::try_parse_from(["prtop", "."]).unwrap();
        assert_eq!(current.scope.as_deref(), Some("."));

        let other = Cli::try_parse_from(["prtop", "~/dev/prtop"]).unwrap();
        assert_eq!(other.scope.as_deref(), Some("~/dev/prtop"));

        let global = Cli::try_parse_from(["prtop", "--global"]).unwrap();
        assert!(global.global);

        let demo = Cli::try_parse_from(["prtop", "--demo"]).unwrap();
        assert!(demo.demo);
    }

    #[test]
    fn cli_help_shows_cargo_argument_forwarding_and_binary_examples() {
        let help = Cli::command().render_long_help().to_string();
        assert!(help.contains("cargo run -- ."));
        assert!(help.contains("cargo run -- ~/dev/prtop"));
        assert!(help.contains("cargo run -- --global"));
        assert!(help.contains("prtop ~/dev/prtop"));
        assert!(!help.contains("cargo run prtop"));
    }

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
                AppEvent::CommentWrite { request, correlation_id, result } => app.apply_comment_write(request, correlation_id, result),
                AppEvent::ReviewWrite { request, state, result } => app.apply_review_write(request, state, result),
                AppEvent::LogLoaded { job, chunk } => app.apply_log_chunk(job, chunk),
                AppEvent::PipelinesLoaded { request, pipelines } => app.apply_pipelines(request, pipelines),
                AppEvent::DetailRequestLoaded { request, result } => app.apply_detail_request(request, result),
                AppEvent::CommentsLoaded { request, revision, result } => app.apply_comments(request, revision, result),
                AppEvent::ReviewsLoaded { request, result } => app.apply_reviews(request, result),
                AppEvent::PipelineLoaded { id, pipeline } => app.apply_pipeline(id, *pipeline),
                AppEvent::CiActionCompleted { action, result } => app.apply_ci_action(action, result),
                AppEvent::GitPreflightCompleted { op, result } => app.apply_git_preflight(op, result),
                AppEvent::ProjectGitLoaded(result) => app.apply_project_git(result),
                AppEvent::MergePreflightLoaded { id, result } => app.apply_merge_preflight(id, result),
                AppEvent::GitBranchesLoaded { token, branches } => app.apply_git_branches(token, branches),
                AppEvent::RepositoryInfoLoaded { forge, repository, result } => app.apply_repository_info(forge, repository, result),
                AppEvent::PushCompleted { op, branch, result } => app.apply_push(op, branch, result),
                AppEvent::PickerLoaded { token, result } => app.apply_picker(token, result),
                AppEvent::CreateCompleted { op, result } => app.apply_create(op, result),
                AppEvent::RequestWriteCompleted { id, op, action, result } => app.apply_request_write(id, op, action, result),
                AppEvent::MetadataWriteCompleted { id, op, kind, result } => app.apply_metadata_write(id, op, kind, result),
                AppEvent::MergeCompleted { id, op, result } => app.apply_merge(id, op, result),
                AppEvent::AutoMergeCompleted { id, op, enabled, result } => app.apply_auto_merge(id, op, enabled, result),
                AppEvent::TargetedRequestLoaded { id, result } => app.apply_targeted_request(id, result),
                AppEvent::BranchCleanupCompleted { id, message, result } => app.apply_branch_cleanup(id, message, result),
            }
        }
    }
}
