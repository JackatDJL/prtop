use crate::{
    cache,
    config::{Config, ForgeKind},
    forge::{self, ForgeProvider},
    model::*,
};
use anyhow::Result;
use chrono::Utc;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use std::collections::HashMap;
use std::{sync::Arc, time::Instant};
use tokio::sync::mpsc;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Scope {
    Exact(String),
    Project { host: String, repository: String },
}

pub enum AppEvent {
    Refresh(RefreshResult),
    CommentWrite {
        temporary_id: String,
        result: Result<(), forge::ForgeError>,
    },
    ReviewWrite {
        state: ReviewState,
        result: Result<(), forge::ForgeError>,
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Focus {
    Requests,
    Details,
    Comments,
    Ci,
    Reviewers,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum View {
    Dashboard,
    ChangeRequestDetail(ChangeRequestId),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Overlay {
    Composer { body: String },
    ReviewMenu { selected: usize },
    Palette { query: String, selected: usize },
    ConfirmDelete,
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
    pub view: View,
    pub health: Vec<(String, String)>,
    pub last_refresh: Option<Instant>,
    pub stale: bool,
    pub focus: Focus,
    pub comment_scroll: usize,
    pub ci_scroll: usize,
    pub regions: HitRegions,
    pub toast: Option<String>,
    pub overlay: Option<Overlay>,
    config: Config,
    demo: bool,
    scope: Option<Scope>,
    events: mpsc::UnboundedSender<AppEvent>,
    providers: HashMap<String, Arc<dyn ForgeProvider>>,
}

impl App {
    pub async fn new(
        config: Config,
        demo: bool,
        scope: Option<Scope>,
        events: mpsc::UnboundedSender<AppEvent>,
    ) -> Result<Self> {
        let requests = if demo {
            forge::demo::change_requests()
        } else {
            cache::load().unwrap_or_default()
        };
        let providers = providers(&config);
        Ok(Self {
            requests,
            selected: 0,
            filter: String::new(),
            filtering: false,
            show_help: false,
            view: View::Dashboard,
            health: vec![],
            last_refresh: None,
            stale: !demo,
            focus: Focus::Requests,
            comment_scroll: 0,
            ci_scroll: 0,
            regions: HitRegions::default(),
            toast: None,
            overlay: None,
            config,
            demo,
            scope,
            events,
            providers,
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
    pub fn active_request(&self) -> Option<&ChangeRequest> {
        match &self.view {
            View::Dashboard => self.selected_request(),
            View::ChangeRequestDetail(id) => self.requests.iter().find(|request| request.id == *id),
        }
    }
    #[cfg(test)]
    pub fn handle_key(&mut self, key: KeyCode) -> bool {
        self.handle_key_event(KeyEvent::new(key, KeyModifiers::NONE))
    }
    pub fn handle_key_event(&mut self, event: KeyEvent) -> bool {
        let key = event.code;
        if let Some(overlay) = self.overlay.take() {
            match overlay {
                Overlay::Composer { mut body } => match key {
                    KeyCode::Esc => self.toast = Some("Comment discarded".into()),
                    KeyCode::Backspace => {
                        body.pop();
                        self.overlay = Some(Overlay::Composer { body });
                    }
                    KeyCode::Enter if event.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.submit_comment(body)
                    }
                    KeyCode::Enter => {
                        body.push('\n');
                        self.overlay = Some(Overlay::Composer { body });
                    }
                    KeyCode::Char(c) => {
                        body.push(c);
                        self.overlay = Some(Overlay::Composer { body });
                    }
                    _ => self.overlay = Some(Overlay::Composer { body }),
                },
                Overlay::ReviewMenu { mut selected } => match key {
                    KeyCode::Esc => {}
                    KeyCode::Up | KeyCode::Char('k') => {
                        selected = selected.saturating_sub(1);
                        self.overlay = Some(Overlay::ReviewMenu { selected });
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        selected = (selected + 1).min(2);
                        self.overlay = Some(Overlay::ReviewMenu { selected });
                    }
                    KeyCode::Enter => match selected {
                        0 => self.apply_review(ReviewState::Approved),
                        1 => self.apply_review(ReviewState::ChangesRequested),
                        _ => {
                            self.overlay = Some(Overlay::Composer {
                                body: String::new(),
                            })
                        }
                    },
                    _ => self.overlay = Some(Overlay::ReviewMenu { selected }),
                },
                Overlay::Palette {
                    mut query,
                    mut selected,
                } => match key {
                    KeyCode::Esc => {}
                    KeyCode::Backspace => {
                        query.pop();
                        self.overlay = Some(Overlay::Palette { query, selected });
                    }
                    KeyCode::Char(c) => {
                        query.push(c);
                        selected = 0;
                        self.overlay = Some(Overlay::Palette { query, selected });
                    }
                    KeyCode::Down => {
                        selected = (selected + 1).min(5);
                        self.overlay = Some(Overlay::Palette { query, selected });
                    }
                    KeyCode::Up => {
                        selected = selected.saturating_sub(1);
                        self.overlay = Some(Overlay::Palette { query, selected });
                    }
                    KeyCode::Enter => self.run_palette(selected),
                    _ => self.overlay = Some(Overlay::Palette { query, selected }),
                },
                Overlay::ConfirmDelete => match key {
                    KeyCode::Char('d') => {
                        self.toast = Some(
                            "Delete is capability-gated and not available in this build".into(),
                        )
                    }
                    KeyCode::Esc | KeyCode::Enter => {}
                    _ => self.overlay = Some(Overlay::ConfirmDelete),
                },
            }
            return false;
        }
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
            KeyCode::Char('q') if self.view == View::Dashboard => return true,
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Char('h')
                if self.view != View::Dashboard =>
            {
                self.view = View::Dashboard;
                self.focus = Focus::Requests;
            }
            KeyCode::Char('j') | KeyCode::Down if self.focus == Focus::Comments => {
                self.comment_scroll = self.comment_scroll.saturating_add(1)
            }
            KeyCode::Char('k') | KeyCode::Up if self.focus == Focus::Comments => {
                self.comment_scroll = self.comment_scroll.saturating_sub(1)
            }
            KeyCode::PageDown if self.focus == Focus::Comments => {
                self.comment_scroll = self.comment_scroll.saturating_add(10)
            }
            KeyCode::PageUp if self.focus == Focus::Comments => {
                self.comment_scroll = self.comment_scroll.saturating_sub(10)
            }
            KeyCode::Home if self.focus == Focus::Comments => {
                self.comment_scroll = self
                    .active_request()
                    .map(|pr| pr.comments.len().saturating_sub(10))
                    .unwrap_or(0)
            }
            KeyCode::End if self.focus == Focus::Comments => self.comment_scroll = 0,
            KeyCode::Char('j') | KeyCode::Down if self.focus == Focus::Ci => {
                self.ci_scroll = self.ci_scroll.saturating_add(1)
            }
            KeyCode::Char('k') | KeyCode::Up if self.focus == Focus::Ci => {
                self.ci_scroll = self.ci_scroll.saturating_sub(1)
            }
            KeyCode::Char('j') | KeyCode::Down if self.view == View::Dashboard => {
                if self.selected + 1 < self.visible().len() {
                    self.selected += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up if self.view == View::Dashboard => {
                self.selected = self.selected.saturating_sub(1)
            }
            KeyCode::Enter | KeyCode::Char('l') => self.open_selected(),
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
            KeyCode::Char('c') if self.can(|capabilities| capabilities.comments) => {
                self.overlay = Some(Overlay::Composer {
                    body: String::new(),
                })
            }
            KeyCode::Char('a') if self.can(|capabilities| capabilities.approve) => {
                self.apply_review(ReviewState::Approved)
            }
            KeyCode::Char('x') if self.can(|capabilities| capabilities.request_changes) => {
                self.apply_review(ReviewState::ChangesRequested)
            }
            KeyCode::Char('R') if self.can(|capabilities| capabilities.reviews) => {
                self.overlay = Some(Overlay::ReviewMenu { selected: 0 })
            }
            KeyCode::Char('c') | KeyCode::Char('R') | KeyCode::Char('a') | KeyCode::Char('x') => {
                self.toast = Some("This forge has not advertised this write capability".into())
            }
            KeyCode::Char(':') => {
                self.overlay = Some(Overlay::Palette {
                    query: String::new(),
                    selected: 0,
                })
            }
            KeyCode::Char('d') => self.overlay = Some(Overlay::ConfirmDelete),
            KeyCode::Esc | KeyCode::Char('h') => {
                self.show_help = false;
            }
            _ => {}
        };
        false
    }
    fn open_selected(&mut self) {
        if let Some(request) = self.selected_request() {
            self.view = View::ChangeRequestDetail(request.id.clone());
            self.focus = Focus::Comments;
            self.comment_scroll = 0;
            self.ci_scroll = 0;
        }
    }
    fn can(&self, predicate: impl Fn(forge::ForgeCapabilities) -> bool) -> bool {
        self.demo
            || self
                .active_request()
                .and_then(|request| self.providers.get(&request.id.forge))
                .is_some_and(|provider| predicate(provider.capabilities()))
    }
    fn submit_comment(&mut self, body: String) {
        if body.trim().is_empty() {
            self.toast = Some("Comment is empty".into());
            return;
        }
        let Some(request) = self.active_request().map(|item| item.id.clone()) else {
            return;
        };
        let temporary_id = format!(
            "pending-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        if let Some(pr) = self.requests.iter_mut().find(|pr| pr.id == request) {
            pr.comments.push(Comment {
                id: temporary_id.clone(),
                author: Person {
                    login: "jack".into(),
                    name: Some("Jack".into()),
                },
                body: body.clone(),
                created_at: Utc::now(),
                updated_at: None,
                can_edit: true,
                can_delete: true,
                url: None,
                resolved: None,
            });
        }
        self.comment_scroll = 0;
        if self.demo {
            self.toast = Some("Comment posted".into());
            return;
        }
        let Some(provider) = self.providers.get(&request.forge).cloned() else {
            self.toast = Some("No provider is configured for this request".into());
            return;
        };
        let sender = self.events.clone();
        tokio::spawn(async move {
            let result = provider.create_comment(&request, &body).await;
            let _ = sender.send(AppEvent::CommentWrite {
                temporary_id,
                result,
            });
        });
        self.toast = Some("Sending comment".into());
    }
    fn apply_review(&mut self, state: ReviewState) {
        let Some(request) = self.active_request().map(|item| item.id.clone()) else {
            return;
        };
        if self.demo {
            if let Some(pr) = self.requests.iter_mut().find(|pr| pr.id == request) {
                pr.review = state;
            }
            self.toast = Some(
                match state {
                    ReviewState::Approved => "Review approved",
                    ReviewState::ChangesRequested => "Changes requested",
                    _ => "Review submitted",
                }
                .into(),
            );
            return;
        }
        let Some(provider) = self.providers.get(&request.forge).cloned() else {
            self.toast = Some("No provider is configured for this request".into());
            return;
        };
        let action = match state {
            ReviewState::Approved => forge::ReviewAction::Approve,
            ReviewState::ChangesRequested => forge::ReviewAction::RequestChanges,
            _ => forge::ReviewAction::Comment,
        };
        let sender = self.events.clone();
        tokio::spawn(async move {
            let result = provider.submit_review_action(&request, action, "").await;
            let _ = sender.send(AppEvent::ReviewWrite { state, result });
        });
        self.toast = Some("Submitting review".into());
    }
    fn run_palette(&mut self, selected: usize) {
        match selected {
            0 => {
                self.overlay = Some(Overlay::Composer {
                    body: String::new(),
                })
            }
            1 => self.apply_review(ReviewState::Approved),
            2 => self.apply_review(ReviewState::ChangesRequested),
            3 => self.request_refresh(),
            _ => self.toast = Some("Command unavailable for this forge".into()),
        }
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
                    if row == self.selected {
                        self.open_selected();
                    } else {
                        self.selected = row;
                    }
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
    pub fn apply_comment_write(
        &mut self,
        temporary_id: String,
        result: Result<(), forge::ForgeError>,
    ) {
        match result {
            Ok(()) => {
                self.toast = Some("Comment posted. Refresh to reconcile its server ID.".into())
            }
            Err(error) => {
                self.toast = Some(format!("Comment failed: {error}"));
                for request in &mut self.requests {
                    if let Some(comment) = request
                        .comments
                        .iter_mut()
                        .find(|comment| comment.id == temporary_id)
                    {
                        comment.body = format!("{}\n\n[failed, press c to retry]", comment.body);
                        break;
                    }
                }
            }
        }
    }
    pub fn apply_review_write(
        &mut self,
        state: ReviewState,
        result: Result<(), forge::ForgeError>,
    ) {
        match result {
            Ok(()) => {
                if let Some(id) = self.active_request().map(|request| request.id.clone())
                    && let Some(request) = self.requests.iter_mut().find(|request| request.id == id)
                {
                    request.review = state;
                }
                self.toast = Some("Review submitted".into());
            }
            Err(error) => self.toast = Some(format!("Review failed: {error}")),
        }
    }
}

fn providers(config: &Config) -> HashMap<String, Arc<dyn ForgeProvider>> {
    config
        .forges
        .iter()
        .map(|forge_config| {
            let provider: Arc<dyn ForgeProvider> = match forge_config.kind {
                ForgeKind::Github => Arc::new(forge::github::GitHubProvider::new(
                    forge_config.name.clone(),
                    forge_config.host.clone(),
                    &config.projects,
                )),
                ForgeKind::Gitlab => Arc::new(forge::gitlab::GitLabProvider::new(
                    forge_config.name.clone(),
                    forge_config.host.clone(),
                    &config.projects,
                )),
                ForgeKind::Forgejo => Arc::new(forge::forgejo::ForgejoProvider::new(
                    forge_config.name.clone(),
                    forge_config.host.clone(),
                    &config.projects,
                )),
            };
            (forge_config.name.clone(), provider)
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
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

    #[tokio::test]
    async fn wheel_targets_ci_panel_without_scrolling_comments() {
        let (sender, _) = mpsc::unbounded_channel();
        let mut app = App::new(Config::default(), true, None, sender)
            .await
            .unwrap();
        app.set_regions(HitRegions {
            comments: Rect::new(0, 10, 40, 10),
            ci: Rect::new(40, 10, 40, 10),
            ..HitRegions::default()
        });
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 44,
            row: 12,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.comment_scroll, 0);
        assert_eq!(app.ci_scroll, 1);
    }

    #[tokio::test]
    async fn composer_keeps_draft_and_submits_in_demo() {
        let (sender, _) = mpsc::unbounded_channel();
        let mut app = App::new(Config::default(), true, None, sender)
            .await
            .unwrap();
        let previous = app.selected_request().unwrap().comments.len();
        app.handle_key(KeyCode::Char('c'));
        app.handle_key(KeyCode::Char('h'));
        app.handle_key(KeyCode::Char('i'));
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL));
        assert_eq!(app.selected_request().unwrap().comments.len(), previous + 1);
        assert_eq!(app.toast.as_deref(), Some("Comment posted"));
    }

    #[tokio::test]
    async fn review_menu_updates_demo_review_state() {
        let (sender, _) = mpsc::unbounded_channel();
        let mut app = App::new(Config::default(), true, None, sender)
            .await
            .unwrap();
        app.handle_key(KeyCode::Char('R'));
        app.handle_key(KeyCode::Down);
        app.handle_key(KeyCode::Enter);
        assert_eq!(
            app.selected_request().unwrap().review,
            ReviewState::ChangesRequested
        );
    }

    #[tokio::test]
    async fn enter_opens_and_back_returns_to_dashboard() {
        let (sender, _) = mpsc::unbounded_channel();
        let mut app = App::new(Config::default(), true, None, sender)
            .await
            .unwrap();
        let id = app.selected_request().unwrap().id.clone();
        app.handle_key(KeyCode::Enter);
        assert_eq!(app.view, View::ChangeRequestDetail(id));
        app.handle_key(KeyCode::Esc);
        assert_eq!(app.view, View::Dashboard);
    }

    #[tokio::test]
    async fn detail_view_keeps_the_opened_request_when_selection_changes() {
        let (sender, _) = mpsc::unbounded_channel();
        let mut app = App::new(Config::default(), true, None, sender)
            .await
            .unwrap();
        app.selected = 1;
        let opened = app.selected_request().unwrap().id.clone();
        app.handle_key(KeyCode::Enter);
        app.selected = 0;
        assert_eq!(app.active_request().unwrap().id, opened);
    }
}
async fn refresh(config: Config, demo: bool, scope: Option<Scope>) -> RefreshResult {
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
    if let Some(scope) = &scope {
        retain_scope(&mut all, scope, &config);
    }
    if !all.is_empty() {
        let _ = cache::store(&all);
        return RefreshResult {
            requests: all,
            health,
            from_cache: false,
        };
    }
    let mut cached = cache::load().unwrap_or_default();
    if let Some(scope) = &scope {
        retain_scope(&mut cached, scope, &config);
    }
    RefreshResult {
        requests: cached,
        health,
        from_cache: true,
    }
}

fn retain_scope(requests: &mut Vec<ChangeRequest>, scope: &Scope, config: &Config) {
    requests.retain(|request| match scope {
        Scope::Exact(scope) => request.id.forge == *scope || request.id.repository == *scope,
        Scope::Project { host, repository } => {
            request.id.repository == *repository
                && config
                    .forges
                    .iter()
                    .any(|forge| forge.name == request.id.forge && forge.host == *host)
        }
    });
}
