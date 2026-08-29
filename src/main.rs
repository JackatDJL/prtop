mod app;
mod cache;
mod config;
mod forge;
mod git;
mod model;
mod ssh;
mod ui;

use std::{io, time::Duration};

use anyhow::Result;
use app::{App, AppEvent};
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyEventKind},
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
    /// Limit the initial view to a configured project, forge, repository, or PR URL.
    scope: Option<String>,
}

struct TerminalGuard;
impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen)?;
        Ok(Self)
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "prtop=info".into()))
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let config = config::Config::load_or_create()?;
    let (events, mut receiver) = mpsc::unbounded_channel();
    let mut app = App::new(config, cli.demo, cli.scope, events.clone()).await?;
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
                    if let Event::Key(key) = event::read()? {
                        if key.kind == KeyEventKind::Press && app.handle_key(key.code) { return Ok(()); }
                    }
                }
            }
            Some(message) = receiver.recv() => match message {
                AppEvent::Refresh(result) => app.apply_refresh(result),
            }
        }
    }
}
