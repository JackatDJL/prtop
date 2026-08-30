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
use app::{App, AppEvent};
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
            if !config.forges.iter().any(|forge| forge.host == *host) && host == "github.com" {
                config.forges.push(config::ForgeConfig {
                    name: "github".into(),
                    kind: config::ForgeKind::Github,
                    host: host.clone(),
                });
                config.projects.push(config::ProjectConfig {
                    name: repository.clone(),
                    forge: "github".into(),
                    repo: repository.clone(),
                    path: Some(path.clone()),
                    host: None,
                });
            } else if !config.forges.iter().any(|forge| forge.host == *host) {
                startup_notice = Some(format!(
                    "Repository detected: {host}/{repository}. No forge configuration matches {host}."
                ));
            }
            Some(repository.clone())
        }
        _ => cli.scope.clone(),
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
                AppEvent::ReviewWrite { state, result } => app.apply_review_write(state, result),
            }
        }
    }
}
