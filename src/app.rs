use crate::{
    cache,
    config::{Config, ForgeKind},
    forge::{self, ForgeProvider},
    model::*,
};
use anyhow::Result;
use crossterm::event::{KeyCode, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use std::{sync::Arc, time::Instant};
use tokio::sync::mpsc;

pub enum AppEvent {
    Refresh(RefreshResult),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Focus {
    Requests,
    Details,
    Comments,
    Ci,
    Reviewers,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct HitRegions {
    pub requests: Rect,
    pub details: Rect,
    pub comments: Rect,
    pub ci: Rect,
    pub reviewers: Rect,
}
pub struct RefreshResult {
    pub requests: Vec<ChangeRequest>,
    pub health: Vec<(String, String)>,
    pub from_cache: bool,
}
pub struct App {
    pub requests: Vec<ChangeRequest>,
    pub selected: usize,
    pub filter: String,
    pub filtering: bool,
    pub show_help: bool,
    pub detail: bool,
    pub health: Vec<(String, String)>,
    pub last_refresh: Option<Instant>,
    pub stale: bool,
    pub focus: Focus,
    pub comment_scroll: usize,
    pub ci_scroll: usize,
    pub regions: HitRegions,
    pub toast: Option<String>,
    config: Config,
    demo: bool,
    scope: Option<String>,
    events: mpsc::UnboundedSender<AppEvent>,
}

impl App {
    pub async fn new(
        config: Config,
        demo: bool,
        scope: Option<String>,
        events: mpsc::UnboundedSender<AppEvent>,
    ) -> Result<Self> {
        let requests = if demo {
            forge::demo::change_requests()
        } else {
            cache::load().unwrap_or_default()
        };
        Ok(Self {
            requests,
            selected: 0,
            filter: String::new(),
            filtering: false,
            show_help: false,
            detail: false,
            health: vec![],
            last_refresh: None,
            stale: !demo,
            focus: Focus::Requests,
            comment_scroll: 0,
            ci_scroll: 0,
            regions: HitRegions::default(),
            toast: None,
            config,
            demo,
            scope,
            events,
        })
    }
    pub fn visible(&self) -> Vec<&ChangeRequest> {
        self.requests
            .iter()
            .filter(|p| {
                let haystack = format!(
                    "{} {} {} {}",
                    p.id.forge, p.id.repository, p.title, p.author.login
                )
                .to_lowercase();
                self.filter.is_empty() || haystack.contains(&self.filter.to_lowercase())
            })
            .collect()
    }
    pub fn selected_request(&self) -> Option<&ChangeRequest> {
        self.visible().get(self.selected).copied()
    }
    pub fn handle_key(&mut self, key: KeyCode) -> bool {
        if self.filtering {
            match key {
                KeyCode::Esc | KeyCode::Enter => self.filtering = false,
                KeyCode::Backspace => {
                    self.filter.pop();
                    self.selected = 0;
                }
                KeyCode::Char(c) => {
                    self.filter.push(c);
                    self.selected = 0;
                }
                _ => {}
            };
            return false;
        }
        match key {
            KeyCode::Char('q') => return true,
            KeyCode::Char('j') | KeyCode::Down => {
                if self.selected + 1 < self.visible().len() {
                    self.selected += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Enter | KeyCode::Char('l') => self.detail = !self.detail,
            KeyCode::Char('/') => self.filtering = true,
            KeyCode::Char('r') => self.request_refresh(),
            KeyCode::Char('?') => self.show_help = !self.show_help,
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Requests => Focus::Details,
                    Focus::Details => Focus::Ci,
                    Focus::Ci => Focus::Reviewers,
                    Focus::Reviewers => Focus::Comments,
                    Focus::Comments => Focus::Requests,
                }
            }
            KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::Requests => Focus::Comments,
                    Focus::Details => Focus::Requests,
                    Focus::Ci => Focus::Details,
                    Focus::Reviewers => Focus::Ci,
                    Focus::Comments => Focus::Reviewers,
                }
            }
            KeyCode::Char('c') => {
                self.toast = Some("Comment editor is coming next in this slice".into())
            }
            KeyCode::Esc | KeyCode::Char('h') => {
                self.detail = false;
                self.show_help = false;
            }
            _ => {}
        };
        false
    }
    pub fn set_regions(&mut self, regions: HitRegions) {
        self.regions = regions;
    }
    pub fn handle_mouse(&mut self, event: MouseEvent) {
        let point = |rect: Rect| {
            event.column >= rect.x
                && event.column < rect.x + rect.width
                && event.row >= rect.y
                && event.row < rect.y + rect.height
        };
        match event.kind {
            MouseEventKind::Down(_) if point(self.regions.requests) => {
                self.focus = Focus::Requests;
                let row = event.row.saturating_sub(self.regions.requests.y + 1) as usize;
                if row < self.visible().len() {
                    self.selected = row;
                }
            }
            MouseEventKind::Down(_) if point(self.regions.details) => self.focus = Focus::Details,
            MouseEventKind::Down(_) if point(self.regions.comments) => self.focus = Focus::Comments,
            MouseEventKind::Down(_) if point(self.regions.ci) => self.focus = Focus::Ci,
            MouseEventKind::Down(_) if point(self.regions.reviewers) => {
                self.focus = Focus::Reviewers
            }
            MouseEventKind::ScrollUp if point(self.regions.comments) => {
                self.comment_scroll = self.comment_scroll.saturating_sub(3)
            }
            MouseEventKind::ScrollDown if point(self.regions.comments) => {
                self.comment_scroll = self.comment_scroll.saturating_add(3)
            }
            MouseEventKind::ScrollUp if point(self.regions.ci) => {
                self.ci_scroll = self.ci_scroll.saturating_sub(1)
            }
            MouseEventKind::ScrollDown if point(self.regions.ci) => {
                self.ci_scroll = self.ci_scroll.saturating_add(1)
            }
            _ => {}
        }
    }
    pub fn request_refresh(&self) {
        let sender = self.events.clone();
        let config = self.config.clone();
        let demo = self.demo;
        let scope = self.scope.clone();
        tokio::spawn(async move {
            let result = refresh(config, demo, scope).await;
            let _ = sender.send(AppEvent::Refresh(result));
        });
    }
    pub fn apply_refresh(&mut self, result: RefreshResult) {
        self.requests = result.requests;
        self.health = result.health;
        self.stale = result.from_cache;
        self.last_refresh = Some(Instant::now());
        if self.selected >= self.visible().len() {
            self.selected = self.visible().len().saturating_sub(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyModifiers, MouseButton};

    #[tokio::test]
    async fn click_and_keyboard_share_request_selection() {
        let (sender, _) = mpsc::unbounded_channel();
        let mut app = App::new(Config::default(), true, None, sender)
            .await
            .unwrap();
        app.set_regions(HitRegions {
            requests: Rect::new(0, 0, 80, 10),
            ..HitRegions::default()
        });
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 4,
            row: 2,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.selected, 1);
        app.handle_key(KeyCode::Char('j'));
        assert_eq!(app.selected, 2);
    }

    #[tokio::test]
    async fn wheel_targets_comment_panel() {
        let (sender, _) = mpsc::unbounded_channel();
        let mut app = App::new(Config::default(), true, None, sender)
            .await
            .unwrap();
        app.set_regions(HitRegions {
            comments: Rect::new(0, 10, 40, 10),
            ..HitRegions::default()
        });
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 4,
            row: 12,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.focus, Focus::Requests);
        assert_eq!(app.comment_scroll, 3);
    }
}
async fn refresh(config: Config, demo: bool, scope: Option<String>) -> RefreshResult {
    if demo {
        return RefreshResult {
            requests: forge::demo::change_requests(),
            health: vec![
                ("GitHub".into(), "ok".into()),
                ("Volt GitLab".into(), "ok".into()),
                ("Codeberg".into(), "rate limited".into()),
            ],
            from_cache: false,
        };
    }
    let providers: Vec<Arc<dyn ForgeProvider>> = config
        .forges
        .iter()
        .map(|f| match f.kind {
            ForgeKind::Github => Arc::new(forge::github::GitHubProvider::new(
                f.name.clone(),
                f.host.clone(),
                &config.projects,
            )) as Arc<dyn ForgeProvider>,
            ForgeKind::Gitlab => Arc::new(forge::gitlab::GitLabProvider::new(
                f.name.clone(),
                f.host.clone(),
                &config.projects,
            )) as Arc<dyn ForgeProvider>,
            ForgeKind::Forgejo => Arc::new(forge::forgejo::ForgejoProvider::new(
                f.name.clone(),
                f.host.clone(),
                &config.projects,
            )) as Arc<dyn ForgeProvider>,
        })
        .collect();
    let mut all = vec![];
    let mut health = vec![];
    for provider in providers {
        match provider.list_change_requests().await {
            Ok(mut requests) => {
                all.append(&mut requests);
                health.push((provider.name().into(), "ok".into()));
            }
            Err(error) => health.push((provider.name().into(), error.to_string())),
        }
    }
    if let Some(scope) = scope {
        all.retain(|p| p.id.forge == scope || p.id.repository == scope);
    }
    if !all.is_empty() {
        let _ = cache::store(&all);
        return RefreshResult {
            requests: all,
            health,
            from_cache: false,
        };
    }
    let cached = cache::load().unwrap_or_default();
    RefreshResult {
        requests: cached,
        health,
        from_cache: true,
    }
}
