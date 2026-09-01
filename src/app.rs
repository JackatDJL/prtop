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

pub const PALETTE_COMMANDS: [&str; 14] = [
    "Add comment",
    "Approve",
    "Request changes",
    "Refresh",
    "Request reviewer",
    "Open in browser",
    "Open pipeline",
    "Open job logs",
    "Refresh pipeline",
    "Retry failed job",
    "Retry pipeline",
    "Cancel job",
    "Cancel pipeline",
    "Follow logs",
];

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
        request: ChangeRequestId,
        state: ReviewState,
        result: Result<(), forge::ForgeError>,
    },
    LogLoaded {
        job: JobId,
        chunk: LogChunk,
    },
    PipelinesLoaded {
        request: ChangeRequestId,
        pipelines: Result<Vec<Pipeline>, forge::ForgeError>,
    },
    PipelineLoaded {
        id: PipelineId,
        pipeline: Box<Result<Pipeline, forge::ForgeError>>,
    },
    CiActionCompleted {
        action: CiAction,
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DetailFocus {
    Comments,
    Description,
    Reviewers,
    Ci,
    Metadata,
}
impl DetailFocus {
    fn next(self) -> Self {
        match self {
            Self::Comments => Self::Description,
            Self::Description => Self::Reviewers,
            Self::Reviewers => Self::Ci,
            Self::Ci => Self::Metadata,
            Self::Metadata => Self::Comments,
        }
    }
    fn previous(self) -> Self {
        match self {
            Self::Comments => Self::Metadata,
            Self::Description => Self::Comments,
            Self::Reviewers => Self::Description,
            Self::Ci => Self::Reviewers,
            Self::Metadata => Self::Ci,
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum View {
    Dashboard,
    ChangeRequestDetail(ChangeRequestId),
    PipelineDetail(PipelineId),
    JobDetail(JobId),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Overlay {
    Composer { body: String },
    ReviewMenu { selected: usize },
    Palette { query: String, selected: usize },
    ConfirmDelete,
    ConfirmCi { action: CiAction },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CiAction {
    RetryJob(JobId),
    RetryPipeline(PipelineId),
    CancelJob(JobId),
    CancelPipeline(PipelineId),
    PlayJob(JobId),
}
#[derive(Clone, Copy, Debug, Default)]
pub struct HitRegions {
    pub requests: Rect,
    pub details: Rect,
    pub description: Rect,
    pub comments: Rect,
    pub ci: Rect,
    pub reviewers: Rect,
    pub metadata: Rect,
    pub jobs: Rect,
    pub logs: Rect,
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
    pub detail_focus: DetailFocus,
    pub comment_scroll: usize,
    pub ci_scroll: usize,
    pub job_selected: usize,
    pub log_scroll: usize,
    pub follow_logs: bool,
    pub log_query: Option<String>,
    pub log_searching: bool,
    pub logs: HashMap<JobId, Vec<String>>,
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
            detail_focus: DetailFocus::Comments,
            comment_scroll: 0,
            ci_scroll: 0,
            job_selected: 0,
            log_scroll: 0,
            follow_logs: true,
            log_query: None,
            log_searching: false,
            logs: HashMap::new(),
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
    pub fn detail_request(&self) -> Option<&ChangeRequest> {
        let View::ChangeRequestDetail(id) = &self.view else {
            return None;
        };
        self.requests.iter().find(|request| request.id == *id)
    }
    pub fn request_for_view(&self) -> Option<&ChangeRequest> {
        match self.view {
            View::Dashboard => self.selected_request(),
            View::ChangeRequestDetail(_) => self.detail_request(),
            View::PipelineDetail(ref id) => self
                .requests
                .iter()
                .find(|request| request.pipelines.iter().any(|pipeline| pipeline.id == *id)),
            View::JobDetail(ref id) => self.requests.iter().find(|request| {
                request
                    .pipelines
                    .iter()
                    .any(|pipeline| pipeline.id == id.pipeline)
            }),
        }
    }
    pub fn pipeline_for_view(&self) -> Option<&Pipeline> {
        match &self.view {
            View::PipelineDetail(id) => self.pipeline(id),
            View::JobDetail(id) => self.pipeline(&id.pipeline),
            _ => None,
        }
    }
    pub fn pipeline(&self, id: &PipelineId) -> Option<&Pipeline> {
        self.requests
            .iter()
            .flat_map(|request| &request.pipelines)
            .find(|pipeline| pipeline.id == *id)
    }
    pub fn job_for_view(&self) -> Option<&Job> {
        let View::JobDetail(id) = &self.view else {
            return None;
        };
        self.pipeline(&id.pipeline)?
            .jobs
            .iter()
            .find(|job| job.id == *id)
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
                        selected = (selected + 1).min(Self::palette_command_count(&query));
                        self.overlay = Some(Overlay::Palette { query, selected });
                    }
                    KeyCode::Up => {
                        selected = selected.saturating_sub(1);
                        self.overlay = Some(Overlay::Palette { query, selected });
                    }
                    KeyCode::Enter => self.run_palette(&query, selected),
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
                Overlay::ConfirmCi { action } => match key {
                    KeyCode::Enter | KeyCode::Esc => {}
                    KeyCode::Char('y') => self.start_ci_action(action),
                    _ => self.overlay = Some(Overlay::ConfirmCi { action }),
                },
            }
            return false;
        }
        if self.filtering && self.view == View::Dashboard {
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
        if self.log_searching {
            match key {
                KeyCode::Esc | KeyCode::Enter => self.log_searching = false,
                KeyCode::Backspace => {
                    if let Some(query) = &mut self.log_query {
                        query.pop();
                    }
                }
                KeyCode::Char(c) => self.log_query.get_or_insert_with(String::new).push(c),
                _ => {}
            }
            return false;
        }
        match self.view.clone() {
            View::Dashboard => self.handle_dashboard_key(key),
            View::ChangeRequestDetail(id) => self.handle_detail_key(&id, key),
            View::PipelineDetail(id) => self.handle_pipeline_key(&id, key),
            View::JobDetail(id) => self.handle_job_key(&id, key),
        }
    }
    fn handle_dashboard_key(&mut self, key: KeyCode) -> bool {
        match key {
            KeyCode::Char('q') => return true,
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
                    .selected_request()
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
            KeyCode::Char('j') | KeyCode::Down => {
                if self.selected + 1 < self.visible().len() {
                    self.selected += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => self.selected = self.selected.saturating_sub(1),
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
    fn handle_detail_key(&mut self, id: &ChangeRequestId, key: KeyCode) -> bool {
        match key {
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Char('h') => {
                self.view = View::Dashboard;
                self.focus = Focus::Requests;
                self.filtering = false;
            }
            KeyCode::Enter if self.detail_focus == DetailFocus::Ci => self.open_pipeline(),
            KeyCode::Tab => self.detail_focus = self.detail_focus.next(),
            KeyCode::BackTab => self.detail_focus = self.detail_focus.previous(),
            KeyCode::Char('j') | KeyCode::Down => self.scroll_detail(id, 1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_detail(id, -1),
            KeyCode::PageDown => self.scroll_detail(id, 10),
            KeyCode::PageUp => self.scroll_detail(id, -10),
            KeyCode::Home => self.home_detail(id),
            KeyCode::End => self.end_detail(),
            KeyCode::Char('c') if self.can(|capabilities| capabilities.comments) => {
                self.overlay = Some(Overlay::Composer {
                    body: String::new(),
                })
            }
            KeyCode::Char('R') if self.can(|capabilities| capabilities.reviews) => {
                self.overlay = Some(Overlay::ReviewMenu { selected: 0 })
            }
            KeyCode::Char('a') if self.can(|capabilities| capabilities.approve) => {
                self.apply_review(ReviewState::Approved)
            }
            KeyCode::Char('x') if self.can(|capabilities| capabilities.request_changes) => {
                self.apply_review(ReviewState::ChangesRequested)
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
            _ => {}
        }
        false
    }
    fn open_pipeline(&mut self) {
        let selected = self
            .detail_request()
            .and_then(|request| request.pipelines.get(self.ci_scroll))
            .map(|pipeline| pipeline.id.clone());
        if let Some(id) = selected {
            self.view = View::PipelineDetail(id);
            self.job_selected = 0;
            self.load_pipeline();
        } else {
            self.toast = Some("No pipeline reported".into());
        }
    }
    fn handle_pipeline_key(&mut self, id: &PipelineId, key: KeyCode) -> bool {
        match key {
            KeyCode::Esc | KeyCode::Char('h') => {
                if let Some(request) = self.request_for_view().map(|request| request.id.clone()) {
                    self.view = View::ChangeRequestDetail(request);
                }
            }
            KeyCode::Char('q') => {
                if let Some(request) = self.request_for_view().map(|request| request.id.clone()) {
                    self.view = View::ChangeRequestDetail(request);
                }
            }
            KeyCode::Char('j') | KeyCode::Down => {
                let count = self.pipeline(id).map_or(0, |pipeline| pipeline.jobs.len());
                self.job_selected = (self.job_selected + 1).min(count.saturating_sub(1));
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.job_selected = self.job_selected.saturating_sub(1)
            }
            KeyCode::Enter => {
                if let Some(job) = self
                    .pipeline(id)
                    .and_then(|pipeline| pipeline.jobs.get(self.job_selected))
                    .map(|job| job.id.clone())
                {
                    self.open_job(job);
                }
            }
            KeyCode::Char('r') => self.load_pipeline(),
            KeyCode::Char('R') => {
                self.overlay = Some(Overlay::ConfirmCi {
                    action: CiAction::RetryPipeline(id.clone()),
                })
            }
            KeyCode::Char('x') => {
                self.overlay = Some(Overlay::ConfirmCi {
                    action: CiAction::CancelPipeline(id.clone()),
                })
            }
            KeyCode::Char('p') => {
                if let Some(job) = self
                    .pipeline(id)
                    .and_then(|pipeline| pipeline.jobs.get(self.job_selected))
                    .filter(|job| job.status == PipelineStatus::Manual)
                    .map(|job| job.id.clone())
                {
                    self.overlay = Some(Overlay::ConfirmCi {
                        action: CiAction::PlayJob(job),
                    });
                }
            }
            _ => {}
        }
        false
    }
    fn open_job(&mut self, id: JobId) {
        self.view = View::JobDetail(id.clone());
        self.log_scroll = 0;
        self.follow_logs = true;
        if self.demo {
            self.logs.entry(id.clone()).or_insert_with(|| demo_log(&id));
            return;
        }
        let Some(provider) = self.providers.get(&id.pipeline.forge).cloned() else {
            self.toast = Some("No provider is configured for this job".into());
            return;
        };
        let sender = self.events.clone();
        let job = id.clone();
        tokio::spawn(async move {
            if let Ok(chunk) = provider.get_job_log(&job, 0).await {
                let _ = sender.send(AppEvent::LogLoaded { job, chunk });
            }
        });
    }
    fn handle_job_key(&mut self, id: &JobId, key: KeyCode) -> bool {
        match key {
            KeyCode::Esc | KeyCode::Char('h') | KeyCode::Char('q') => {
                self.view = View::PipelineDetail(id.pipeline.clone())
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.log_scroll = self.log_scroll.saturating_add(1);
                self.follow_logs = false;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.log_scroll = self.log_scroll.saturating_sub(1);
                self.follow_logs = false;
            }
            KeyCode::PageDown => {
                self.log_scroll = self.log_scroll.saturating_add(15);
                self.follow_logs = false;
            }
            KeyCode::PageUp => {
                self.log_scroll = self.log_scroll.saturating_sub(15);
                self.follow_logs = false;
            }
            KeyCode::Char('f') => {
                self.follow_logs = !self.follow_logs;
                if self.follow_logs {
                    self.log_scroll = 0;
                }
            }
            KeyCode::Char('/') => {
                self.log_searching = true;
                self.log_query = Some(String::new());
            }
            KeyCode::Char('n') => self.find_log(id, false),
            KeyCode::Char('N') => self.find_log(id, true),
            KeyCode::Char('R') => {
                self.overlay = Some(Overlay::ConfirmCi {
                    action: CiAction::RetryJob(id.clone()),
                })
            }
            KeyCode::Char('x') => {
                self.overlay = Some(Overlay::ConfirmCi {
                    action: CiAction::CancelJob(id.clone()),
                })
            }
            _ => {}
        }
        false
    }
    fn find_log(&mut self, id: &JobId, previous: bool) {
        let Some(query) = self.log_query.as_deref().filter(|query| !query.is_empty()) else {
            return;
        };
        let lines = self.logs.get(id).cloned().unwrap_or_default();
        let start = self.log_scroll.min(lines.len());
        let mut indexes: Box<dyn Iterator<Item = usize>> = if previous {
            Box::new((0..start).rev())
        } else {
            Box::new(start.saturating_add(1)..lines.len())
        };
        if let Some(index) =
            indexes.find(|index| lines[*index].to_lowercase().contains(&query.to_lowercase()))
        {
            self.log_scroll = lines.len().saturating_sub(index + 1);
        }
    }
    fn start_ci_action(&mut self, action: CiAction) {
        if !self.demo {
            let forge = match &action {
                CiAction::RetryJob(id) | CiAction::CancelJob(id) | CiAction::PlayJob(id) => {
                    &id.pipeline.forge
                }
                CiAction::RetryPipeline(id) | CiAction::CancelPipeline(id) => &id.forge,
            };
            let Some(provider) = self.providers.get(forge).cloned() else {
                self.toast = Some("No provider is configured for this CI action".into());
                return;
            };
            let description = match action {
                CiAction::RetryJob(_) => "Retry job",
                CiAction::RetryPipeline(_) => "Retry pipeline",
                CiAction::CancelJob(_) => "Cancel job",
                CiAction::CancelPipeline(_) => "Cancel pipeline",
                CiAction::PlayJob(_) => "Start manual job",
            };
            let completed_action = action.clone();
            let sender = self.events.clone();
            tokio::spawn(async move {
                let result = match action {
                    CiAction::RetryJob(id) => provider.retry_job(&id).await,
                    CiAction::RetryPipeline(id) => provider.retry_pipeline(&id).await,
                    CiAction::CancelJob(id) => provider.cancel_job(&id).await,
                    CiAction::CancelPipeline(id) => provider.cancel_pipeline(&id).await,
                    CiAction::PlayJob(id) => provider.play_job(&id).await,
                };
                let _ = sender.send(AppEvent::CiActionCompleted {
                    action: completed_action,
                    result,
                });
            });
            self.toast = Some(format!("{description} in progress"));
            return;
        }
        match action {
            CiAction::RetryJob(id) => {
                self.toast = Some("Retry job requested".into());
                self.set_job_status(&id, PipelineStatus::Running);
            }
            CiAction::RetryPipeline(id) => {
                self.toast = Some("Retry pipeline requested".into());
                self.set_pipeline_status(&id, PipelineStatus::Running);
            }
            CiAction::CancelJob(id) => {
                self.toast = Some("Cancel job requested".into());
                self.set_job_status(&id, PipelineStatus::Cancelled);
            }
            CiAction::CancelPipeline(id) => {
                self.toast = Some("Cancel pipeline requested".into());
                self.set_pipeline_status(&id, PipelineStatus::Cancelled);
            }
            CiAction::PlayJob(id) => {
                self.toast = Some("Manual job started".into());
                self.set_job_status(&id, PipelineStatus::Running);
            }
        }
    }
    fn set_pipeline_status(&mut self, id: &PipelineId, status: PipelineStatus) {
        if let Some(pipeline) = self
            .requests
            .iter_mut()
            .flat_map(|request| &mut request.pipelines)
            .find(|pipeline| pipeline.id == *id)
        {
            pipeline.status = status;
        }
    }
    fn set_job_status(&mut self, id: &JobId, status: PipelineStatus) {
        if let Some(job) = self
            .requests
            .iter_mut()
            .flat_map(|request| &mut request.pipelines)
            .find(|pipeline| pipeline.id == id.pipeline)
            .and_then(|pipeline| pipeline.jobs.iter_mut().find(|job| job.id == *id))
        {
            job.status = status;
        }
    }
    fn scroll_detail(&mut self, id: &ChangeRequestId, delta: isize) {
        match self.detail_focus {
            DetailFocus::Comments => {
                let max = self
                    .requests
                    .iter()
                    .find(|request| request.id == *id)
                    .map_or(0, |request| request.comments.len().saturating_sub(10));
                self.comment_scroll = self.comment_scroll.saturating_add_signed(delta).min(max);
            }
            DetailFocus::Ci => {
                let max = self
                    .requests
                    .iter()
                    .find(|request| request.id == *id)
                    .map_or(0, |request| request.pipelines.len().saturating_sub(8));
                self.ci_scroll = self.ci_scroll.saturating_add_signed(delta).min(max);
            }
            DetailFocus::Description | DetailFocus::Reviewers | DetailFocus::Metadata => {}
        }
    }
    fn home_detail(&mut self, id: &ChangeRequestId) {
        match self.detail_focus {
            DetailFocus::Comments => {
                self.comment_scroll = self
                    .requests
                    .iter()
                    .find(|request| request.id == *id)
                    .map_or(0, |request| request.comments.len().saturating_sub(10));
            }
            DetailFocus::Ci => {
                self.ci_scroll = self
                    .requests
                    .iter()
                    .find(|request| request.id == *id)
                    .map_or(0, |request| request.pipelines.len().saturating_sub(8));
            }
            DetailFocus::Description | DetailFocus::Reviewers | DetailFocus::Metadata => {}
        }
    }
    fn end_detail(&mut self) {
        match self.detail_focus {
            DetailFocus::Comments => self.comment_scroll = 0,
            DetailFocus::Ci => self.ci_scroll = 0,
            DetailFocus::Description | DetailFocus::Reviewers | DetailFocus::Metadata => {}
        }
    }
    fn open_selected(&mut self) {
        if let Some(id) = self.selected_request().map(|request| request.id.clone()) {
            self.view = View::ChangeRequestDetail(id.clone());
            self.filtering = false;
            self.detail_focus = DetailFocus::Comments;
            self.comment_scroll = 0;
            self.ci_scroll = 0;
            if self.can(|capabilities| capabilities.ci_read) {
                self.load_pipelines(id);
            }
        }
    }
    fn load_pipelines(&self, request: ChangeRequestId) {
        if self.demo {
            return;
        }
        let Some(provider) = self.providers.get(&request.forge).cloned() else {
            return;
        };
        let sender = self.events.clone();
        tokio::spawn(async move {
            let pipelines = provider.list_pipelines(&request).await;
            let _ = sender.send(AppEvent::PipelinesLoaded { request, pipelines });
        });
    }
    fn load_pipeline(&self) {
        if self.demo {
            return;
        }
        let Some(id) = self.pipeline_for_view().map(|pipeline| pipeline.id.clone()) else {
            return;
        };
        let Some(provider) = self.providers.get(&id.forge).cloned() else {
            return;
        };
        let sender = self.events.clone();
        tokio::spawn(async move {
            let pipeline = provider.get_pipeline(&id).await;
            let _ = sender.send(AppEvent::PipelineLoaded {
                id,
                pipeline: Box::new(pipeline),
            });
        });
    }
    fn can(&self, predicate: impl Fn(forge::ForgeCapabilities) -> bool) -> bool {
        self.demo
            || self
                .request_for_view()
                .and_then(|request| self.providers.get(&request.id.forge))
                .is_some_and(|provider| predicate(provider.capabilities()))
    }
    fn submit_comment(&mut self, body: String) {
        if body.trim().is_empty() {
            self.toast = Some("Comment is empty".into());
            return;
        }
        let Some(request) = self.request_for_view().map(|item| item.id.clone()) else {
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
        if !self.can_review_action(state) {
            self.toast = Some("This forge has not advertised this write capability".into());
            return;
        }
        let Some(request) = self.request_for_view().map(|item| item.id.clone()) else {
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
            let _ = sender.send(AppEvent::ReviewWrite {
                request,
                state,
                result,
            });
        });
        self.toast = Some("Submitting review".into());
    }
    fn can_review_action(&self, state: ReviewState) -> bool {
        match state {
            ReviewState::Approved => self.can(|capabilities| capabilities.approve),
            ReviewState::ChangesRequested => self.can(|capabilities| capabilities.request_changes),
            _ => self.can(|capabilities| capabilities.reviews),
        }
    }
    fn palette_command_count(query: &str) -> usize {
        PALETTE_COMMANDS
            .iter()
            .filter(|command| command.to_lowercase().contains(&query.to_lowercase()))
            .count()
            .saturating_sub(1)
    }
    fn run_palette(&mut self, query: &str, selected: usize) {
        let command = PALETTE_COMMANDS
            .iter()
            .enumerate()
            .filter(|(_, command)| command.to_lowercase().contains(&query.to_lowercase()))
            .nth(selected)
            .map(|(index, _)| index);
        match command {
            Some(0) => {
                self.overlay = Some(Overlay::Composer {
                    body: String::new(),
                })
            }
            Some(1) => self.apply_review(ReviewState::Approved),
            Some(2) => self.apply_review(ReviewState::ChangesRequested),
            Some(3) => self.request_refresh(),
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
        if matches!(self.view, View::ChangeRequestDetail(_)) {
            self.handle_detail_mouse(event, &point);
            return;
        }
        if matches!(self.view, View::PipelineDetail(_)) {
            if matches!(event.kind, MouseEventKind::Down(_))
                && point(self.regions.jobs)
                && event.row > self.regions.jobs.y
            {
                let row = event.row.saturating_sub(self.regions.jobs.y + 1) as usize;
                let pipeline = self.pipeline_for_view();
                if row < pipeline.map_or(0, |pipeline| pipeline.jobs.len()) {
                    if row == self.job_selected {
                        if let Some(id) = pipeline
                            .and_then(|pipeline| pipeline.jobs.get(row))
                            .map(|job| job.id.clone())
                        {
                            self.open_job(id);
                        }
                    } else {
                        self.job_selected = row;
                    }
                }
            }
            return;
        }
        if matches!(self.view, View::JobDetail(_)) {
            if point(self.regions.logs) {
                match event.kind {
                    MouseEventKind::ScrollUp => {
                        self.log_scroll = self.log_scroll.saturating_sub(3);
                        self.follow_logs = false;
                    }
                    MouseEventKind::ScrollDown => {
                        self.log_scroll = self.log_scroll.saturating_add(3);
                        self.follow_logs = false;
                    }
                    _ => {}
                }
            }
            return;
        }
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
    fn handle_detail_mouse(&mut self, event: MouseEvent, point: &impl Fn(Rect) -> bool) {
        match event.kind {
            MouseEventKind::Down(_) if point(self.regions.description) => {
                self.detail_focus = DetailFocus::Description
            }
            MouseEventKind::Down(_) if point(self.regions.comments) => {
                self.detail_focus = DetailFocus::Comments
            }
            MouseEventKind::Down(_) if point(self.regions.reviewers) => {
                self.detail_focus = DetailFocus::Reviewers
            }
            MouseEventKind::Down(_) if point(self.regions.ci) => {
                self.detail_focus = DetailFocus::Ci
            }
            MouseEventKind::Down(_) if point(self.regions.metadata) => {
                self.detail_focus = DetailFocus::Metadata
            }
            MouseEventKind::ScrollUp if point(self.regions.comments) => {
                self.detail_focus = DetailFocus::Comments;
                if let View::ChangeRequestDetail(id) = self.view.clone() {
                    self.scroll_detail(&id, -3);
                }
            }
            MouseEventKind::ScrollDown if point(self.regions.comments) => {
                self.detail_focus = DetailFocus::Comments;
                if let View::ChangeRequestDetail(id) = self.view.clone() {
                    self.scroll_detail(&id, 3);
                }
            }
            MouseEventKind::ScrollUp if point(self.regions.ci) => {
                self.detail_focus = DetailFocus::Ci;
                if let View::ChangeRequestDetail(id) = self.view.clone() {
                    self.scroll_detail(&id, -3);
                }
            }
            MouseEventKind::ScrollDown if point(self.regions.ci) => {
                self.detail_focus = DetailFocus::Ci;
                if let View::ChangeRequestDetail(id) = self.view.clone() {
                    self.scroll_detail(&id, 3);
                }
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
        id: ChangeRequestId,
        state: ReviewState,
        result: Result<(), forge::ForgeError>,
    ) {
        match result {
            Ok(()) => {
                if let Some(request) = self.requests.iter_mut().find(|request| request.id == id) {
                    request.review = state;
                }
                self.toast = Some("Review submitted".into());
            }
            Err(error) => self.toast = Some(format!("Review failed: {error}")),
        }
    }
    pub fn apply_log_chunk(&mut self, job: JobId, chunk: LogChunk) {
        const MAX_LOG_LINES: usize = 30_000;
        let lines = self.logs.entry(job).or_default();
        lines.extend(chunk.text.lines().map(str::to_owned));
        if lines.len() > MAX_LOG_LINES {
            let removed = lines.len() - MAX_LOG_LINES;
            lines.drain(..removed);
            lines.insert(0, "[ older log lines omitted ]".into());
        }
        if self.follow_logs {
            self.log_scroll = 0;
        }
    }
    pub fn apply_pipelines(
        &mut self,
        request: ChangeRequestId,
        result: Result<Vec<Pipeline>, forge::ForgeError>,
    ) {
        match result {
            Ok(pipelines) => {
                if let Some(item) = self.requests.iter_mut().find(|item| item.id == request) {
                    item.ci = pipelines
                        .iter()
                        .map(|pipeline| pipeline.status.ci_state())
                        .find(|status| *status == CiState::Failed || *status == CiState::Running)
                        .unwrap_or_else(|| {
                            pipelines
                                .first()
                                .map(|pipeline| pipeline.status.ci_state())
                                .unwrap_or(CiState::None)
                        });
                    item.pipelines = pipelines;
                }
            }
            Err(error) => self.toast = Some(format!("CI refresh failed: {error}")),
        }
    }
    pub fn apply_pipeline(&mut self, id: PipelineId, result: Result<Pipeline, forge::ForgeError>) {
        match result {
            Ok(pipeline) => {
                if let Some(current) = self
                    .requests
                    .iter_mut()
                    .flat_map(|request| &mut request.pipelines)
                    .find(|current| current.id == id)
                {
                    *current = pipeline;
                }
            }
            Err(error) => self.toast = Some(format!("Pipeline refresh failed: {error}")),
        }
    }
    pub fn apply_ci_action(&mut self, _action: CiAction, result: Result<(), forge::ForgeError>) {
        match result {
            Ok(()) => {
                self.toast = Some("CI action completed. Refreshing pipeline.".into());
                self.load_pipeline();
            }
            Err(error) => self.toast = Some(format!("CI action failed: {error}")),
        }
    }
}

fn demo_log(id: &JobId) -> Vec<String> {
    let mut lines = vec![
        format!("==> job {}", id.value),
        "12:31:04 Running integration tests...".into(),
        "12:31:07 PASS transfer_web".into(),
    ];
    if id.value.ends_with("-1") {
        lines.push("12:31:12 FAIL transfer_mobile".into());
        lines.push("assertion failed: expected connected".into());
    }
    lines.extend(
        (0..250).map(|index| format!("12:32:{:02} test output line {}", index % 60, index + 1)),
    );
    lines
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

    struct TestProvider {
        name: String,
        capabilities: forge::ForgeCapabilities,
    }

    #[async_trait::async_trait]
    impl ForgeProvider for TestProvider {
        fn name(&self) -> &str {
            &self.name
        }

        fn capabilities(&self) -> forge::ForgeCapabilities {
            self.capabilities
        }

        async fn list_change_requests(&self) -> Result<Vec<ChangeRequest>, forge::ForgeError> {
            Ok(vec![])
        }

        async fn submit_review_action(
            &self,
            _id: &ChangeRequestId,
            _action: forge::ReviewAction,
            _body: &str,
        ) -> Result<(), forge::ForgeError> {
            Ok(())
        }
    }

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
    async fn ci_navigation_returns_one_level_at_a_time() {
        let (sender, _) = mpsc::unbounded_channel();
        let mut app = App::new(Config::default(), true, None, sender)
            .await
            .unwrap();
        app.open_selected();
        app.detail_focus = DetailFocus::Ci;
        app.handle_key(KeyCode::Enter);
        assert!(matches!(app.view, View::PipelineDetail(_)));
        app.handle_key(KeyCode::Enter);
        assert!(matches!(app.view, View::JobDetail(_)));
        app.handle_key(KeyCode::Esc);
        assert!(matches!(app.view, View::PipelineDetail(_)));
        app.handle_key(KeyCode::Esc);
        assert!(matches!(app.view, View::ChangeRequestDetail(_)));
    }

    #[tokio::test]
    async fn job_log_scroll_does_not_change_selected_job() {
        let (sender, _) = mpsc::unbounded_channel();
        let mut app = App::new(Config::default(), true, None, sender)
            .await
            .unwrap();
        app.open_selected();
        app.detail_focus = DetailFocus::Ci;
        app.handle_key(KeyCode::Enter);
        app.handle_key(KeyCode::Enter);
        app.handle_key(KeyCode::Down);
        assert_eq!(app.job_selected, 0);
        assert!(app.log_scroll > 0);
        assert!(!app.follow_logs);
    }

    #[tokio::test]
    async fn ci_confirmation_defaults_to_safe_cancel() {
        let (sender, _) = mpsc::unbounded_channel();
        let mut app = App::new(Config::default(), true, None, sender)
            .await
            .unwrap();
        app.open_selected();
        app.detail_focus = DetailFocus::Ci;
        app.handle_key(KeyCode::Enter);
        app.handle_key(KeyCode::Char('x'));
        assert!(matches!(app.overlay, Some(Overlay::ConfirmCi { .. })));
        app.handle_key(KeyCode::Enter);
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn ci_action_error_is_shown_to_the_user() {
        let (sender, _) = mpsc::unbounded_channel();
        let mut app = App::new(Config::default(), true, None, sender)
            .await
            .unwrap();
        let id = app.requests[0].pipelines[0].id.clone();
        app.apply_ci_action(
            CiAction::RetryPipeline(id),
            Err(forge::ForgeError::PermissionDenied),
        );
        assert_eq!(
            app.toast.as_deref(),
            Some("CI action failed: permission denied")
        );
    }

    #[tokio::test]
    async fn clicking_pipeline_border_does_not_select_a_job() {
        let (sender, _) = mpsc::unbounded_channel();
        let mut app = App::new(Config::default(), true, None, sender)
            .await
            .unwrap();
        app.open_selected();
        app.detail_focus = DetailFocus::Ci;
        app.handle_key(KeyCode::Enter);
        app.job_selected = 1;
        app.set_regions(HitRegions {
            jobs: Rect::new(0, 10, 80, 10),
            ..HitRegions::default()
        });
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 4,
            row: 10,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.job_selected, 1);
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
    async fn opening_and_closing_detail_clears_an_active_filter() {
        let (sender, _) = mpsc::unbounded_channel();
        let mut app = App::new(Config::default(), true, None, sender)
            .await
            .unwrap();
        app.filter = "droplet".into();
        app.filtering = true;
        app.set_regions(HitRegions {
            requests: Rect::new(0, 0, 80, 10),
            ..HitRegions::default()
        });

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 4,
            row: 1,
            modifiers: KeyModifiers::NONE,
        });
        app.handle_key(KeyCode::Esc);

        assert_eq!(app.view, View::Dashboard);
        assert!(!app.filtering);
    }

    #[tokio::test]
    async fn review_menu_rejects_actions_the_provider_does_not_advertise() {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let mut app = App::new(Config::default(), true, None, sender)
            .await
            .unwrap();
        app.demo = false;
        let id = app.requests[2].id.clone();
        app.providers.insert(
            id.forge.clone(),
            Arc::new(TestProvider {
                name: id.forge.clone(),
                capabilities: forge::ForgeCapabilities {
                    reviews: true,
                    ..forge::ForgeCapabilities::default()
                },
            }),
        );
        app.selected = 2;
        app.handle_key(KeyCode::Enter);
        app.overlay = Some(Overlay::ReviewMenu { selected: 0 });

        app.handle_key(KeyCode::Enter);

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), receiver.recv())
                .await
                .is_err()
        );
        assert_eq!(
            app.toast.as_deref(),
            Some("This forge has not advertised this write capability")
        );

        app.handle_key(KeyCode::Char(':'));
        for key in "approve".chars() {
            app.handle_key(KeyCode::Char(key));
        }
        app.handle_key(KeyCode::Enter);

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), receiver.recv())
                .await
                .is_err()
        );
        assert_eq!(
            app.toast.as_deref(),
            Some("This forge has not advertised this write capability")
        );
    }

    #[tokio::test]
    async fn palette_dispatches_the_selected_filtered_command() {
        let (sender, _receiver) = mpsc::unbounded_channel();
        let mut app = App::new(Config::default(), true, None, sender)
            .await
            .unwrap();
        app.handle_key(KeyCode::Enter);
        app.handle_key(KeyCode::Char(':'));
        for key in "refresh".chars() {
            app.handle_key(KeyCode::Char(key));
        }

        app.handle_key(KeyCode::Enter);

        assert!(!matches!(app.overlay, Some(Overlay::Composer { .. })));
    }

    #[tokio::test]
    async fn review_write_completion_updates_the_request_that_was_submitted() {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let mut app = App::new(Config::default(), true, None, sender)
            .await
            .unwrap();
        app.demo = false;
        let submitted_id = app.requests[2].id.clone();
        app.providers.insert(
            submitted_id.forge.clone(),
            Arc::new(TestProvider {
                name: submitted_id.forge.clone(),
                capabilities: forge::ForgeCapabilities {
                    approve: true,
                    ..forge::ForgeCapabilities::default()
                },
            }),
        );
        app.selected = 2;
        app.apply_review(ReviewState::Approved);
        app.selected = 3;

        let AppEvent::ReviewWrite {
            request,
            state,
            result,
        } = receiver.recv().await.unwrap()
        else {
            panic!("expected a review completion");
        };
        app.apply_review_write(request, state, result);

        assert_eq!(
            app.requests
                .iter()
                .find(|request| request.id == submitted_id)
                .unwrap()
                .review,
            ReviewState::Approved
        );
        assert_eq!(app.requests[3].review, ReviewState::None);
    }

    #[tokio::test]
    async fn detail_arrows_scroll_comments_without_changing_dashboard_selection() {
        let (sender, _) = mpsc::unbounded_channel();
        let mut app = App::new(Config::default(), true, None, sender)
            .await
            .unwrap();
        app.selected = 3;
        let id = app.selected_request().unwrap().id.clone();

        app.handle_key(KeyCode::Enter);
        app.handle_key(KeyCode::Down);

        assert_eq!(app.selected, 3);
        assert_eq!(app.view, View::ChangeRequestDetail(id));
        assert_eq!(app.comment_scroll, 1);
    }

    #[tokio::test]
    async fn detail_request_is_stable_when_dashboard_selection_changes() {
        let (sender, _) = mpsc::unbounded_channel();
        let mut app = App::new(Config::default(), true, None, sender)
            .await
            .unwrap();
        let id = app.selected_request().unwrap().id.clone();
        app.handle_key(KeyCode::Enter);
        app.selected = 1;

        assert_eq!(app.detail_request().unwrap().id, id);
    }

    #[tokio::test]
    async fn detail_tab_cycles_panels_and_escape_preserves_selection() {
        let (sender, _) = mpsc::unbounded_channel();
        let mut app = App::new(Config::default(), true, None, sender)
            .await
            .unwrap();
        app.selected = 2;
        app.handle_key(KeyCode::Enter);

        assert_eq!(app.detail_focus, DetailFocus::Comments);
        app.handle_key(KeyCode::Tab);
        assert_eq!(app.detail_focus, DetailFocus::Description);
        app.handle_key(KeyCode::Tab);
        assert_eq!(app.detail_focus, DetailFocus::Reviewers);
        app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(app.detail_focus, DetailFocus::Description);
        app.handle_key(KeyCode::Esc);

        assert_eq!(app.view, View::Dashboard);
        assert_eq!(app.selected, 2);
    }

    #[tokio::test]
    async fn detail_comment_wheel_does_not_change_dashboard_selection() {
        let (sender, _) = mpsc::unbounded_channel();
        let mut app = App::new(Config::default(), true, None, sender)
            .await
            .unwrap();
        app.selected = 3;
        app.handle_key(KeyCode::Enter);
        app.set_regions(HitRegions {
            comments: Rect::new(0, 10, 40, 10),
            requests: Rect::new(0, 0, 80, 10),
            ..HitRegions::default()
        });

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 4,
            row: 12,
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(app.selected, 3);
        assert_eq!(app.comment_scroll, 3);
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
