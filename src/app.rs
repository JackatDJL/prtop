use crate::{
    cache,
    config::{Config, ForgeKind},
    create::{self, CreateWorkflow},
    edit::{self, EditAction, EditSession},
    forge::{self, ForgeProvider},
    git::repo::{self, BranchState, PushError, RepoContext},
    merge::MergeSession,
    model::*,
    picker::{PickerItem, PickerKind, PickerSession},
    write::OpId,
};
use anyhow::Result;
use chrono::Utc;
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use std::collections::HashMap;
use std::{
    future::Future,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

pub const PALETTE_COMMANDS: [&str; 29] = [
    "Create pull request",
    "Edit",
    "Edit title",
    "Edit description",
    "Edit labels",
    "Edit reviewers",
    "Edit assignees",
    "Edit milestone",
    "Mark as draft",
    "Mark ready for review",
    "Close request",
    "Reopen request",
    "Merge",
    "Enable auto-merge",
    "Disable auto-merge",
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
        request: ChangeRequestId,
        correlation_id: OpId,
        result: Result<Comment, forge::ForgeError>,
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
    DetailRequestLoaded {
        request: ChangeRequestId,
        result: Result<ChangeRequest, forge::ForgeError>,
    },
    CommentsLoaded {
        request: ChangeRequestId,
        revision: u64,
        result: Result<Vec<Comment>, forge::ForgeError>,
    },
    ReviewsLoaded {
        request: ChangeRequestId,
        result: Result<Vec<Reviewer>, forge::ForgeError>,
    },
    PipelineLoaded {
        id: PipelineId,
        pipeline: Box<Result<Pipeline, forge::ForgeError>>,
    },
    CiActionCompleted {
        action: CiAction,
        result: Result<(), forge::ForgeError>,
    },
    GitPreflightCompleted {
        op: OpId,
        result: Result<GitPreflight, String>,
    },
    MergePreflightLoaded {
        id: ChangeRequestId,
        result: Result<ChangeRequest, forge::ForgeError>,
    },
    ProjectGitLoaded(Result<BranchState, String>),
    GitBranchesLoaded {
        token: OpId,
        branches: Vec<String>,
    },
    RepositoryInfoLoaded {
        forge: String,
        repository: String,
        result: Result<forge::RepositoryInfo, forge::ForgeError>,
    },
    PushCompleted {
        op: OpId,
        branch: String,
        result: Result<(), PushError>,
    },
    PickerLoaded {
        token: OpId,
        result: Result<Vec<PickerItem>, forge::ForgeError>,
    },
    CreateCompleted {
        op: OpId,
        result: Result<forge::CreateResult, forge::ForgeError>,
    },
    RequestWriteCompleted {
        id: ChangeRequestId,
        op: OpId,
        action: LifecycleAction,
        result: Result<ChangeRequest, forge::ForgeError>,
    },
    MetadataWriteCompleted {
        id: ChangeRequestId,
        op: OpId,
        kind: MetaKind,
        result: Result<edit::MetaPayload, forge::ForgeError>,
    },
    MergeCompleted {
        id: ChangeRequestId,
        op: OpId,
        result: Result<MergeOutcome, forge::ForgeError>,
    },
    AutoMergeCompleted {
        id: ChangeRequestId,
        op: OpId,
        enabled: bool,
        result: Result<bool, forge::ForgeError>,
    },
    TargetedRequestLoaded {
        id: ChangeRequestId,
        result: Result<ChangeRequest, forge::ForgeError>,
    },
    BranchCleanupCompleted {
        id: ChangeRequestId,
        message: String,
        result: Result<(), String>,
    },
}

#[derive(Clone, Debug)]
pub struct GitPreflight {
    pub state: BranchState,
    pub remote_exists: Option<bool>,
    pub template: Option<String>,
    pub subjects: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MetaKind {
    Labels,
    Reviewers,
    Assignees,
    Milestone,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleAction {
    Title,
    Body,
    Draft,
    Ready,
    Close,
    Reopen,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfirmAction {
    CloseRequest(ChangeRequestId),
    DeleteRemoteBranch { id: ChangeRequestId, branch: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfirmDialog {
    pub title: String,
    pub body: String,
    pub confirm: String,
    pub cancel: String,
    pub danger: bool,
    /// Zero is always the safe, cancel action.
    pub selected: usize,
    pub action: ConfirmAction,
    pub button_hits: Vec<(Rect, usize)>,
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
#[derive(Clone, Debug)]
pub enum Overlay {
    Composer {
        body: String,
        error: Option<String>,
        retry_requires_refresh: bool,
        button_hits: Vec<(Rect, usize)>,
    },
    ReviewMenu {
        selected: usize,
    },
    Palette {
        query: String,
        selected: usize,
    },
    ConfirmDelete,
    ConfirmCi {
        action: CiAction,
    },
    Create(Box<CreateWorkflow>),
    Edit(EditSession),
    Merge(MergeSession),
    Confirm(ConfirmDialog),
    BranchCleanup(BranchCleanupSession),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BranchCleanupSession {
    pub id: ChangeRequestId,
    pub kind: ChangeRequestKind,
    pub branch: String,
    pub root: Option<std::path::PathBuf>,
    pub remote: String,
    pub choices: Vec<BranchCleanupChoice>,
    /// Keep is index zero and remains the default.
    pub selected: usize,
    pub pending: bool,
    pub button_hits: Vec<(Rect, usize)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BranchCleanupChoice {
    KeepBranches,
    DeleteRemote,
    DeleteLocal,
    DeleteBoth,
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

#[derive(Clone, Debug)]
pub struct DetailResources {
    pub request: LoadState<()>,
    pub comments: LoadState<Vec<Comment>>,
    pub reviews: LoadState<Vec<Reviewer>>,
    pub ci: LoadState<Vec<Pipeline>>,
    comments_revision: u64,
    refreshed_at: Option<Instant>,
}

impl Default for DetailResources {
    fn default() -> Self {
        Self {
            request: LoadState::NotLoaded,
            comments: LoadState::NotLoaded,
            reviews: LoadState::NotLoaded,
            ci: LoadState::NotLoaded,
            comments_revision: 0,
            refreshed_at: None,
        }
    }
}

#[derive(Clone, Debug)]
struct PendingComment {
    request: ChangeRequestId,
    correlation_id: OpId,
    body: String,
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
    toast: Option<String>,
    pub overlay: Option<Overlay>,
    pub repo_context: RepoContext,
    pub project_git: Option<BranchState>,
    pub palette_hits: Vec<(Rect, usize)>,
    pub activity: HashMap<ChangeRequestId, Vec<String>>,
    pub detail_resources: HashMap<ChangeRequestId, DetailResources>,
    repository_info: HashMap<(String, String), forge::RepositoryInfo>,
    repository_info_loading: std::collections::HashSet<(String, String)>,
    repository_info_failed: std::collections::HashSet<(String, String)>,
    config: Config,
    pub(crate) demo: bool,
    scope: Option<Scope>,
    pub(crate) events: mpsc::UnboundedSender<AppEvent>,
    pub(crate) providers: HashMap<String, Arc<dyn ForgeProvider>>,
    pub(crate) last_op: Option<OpId>,
    pub(crate) in_flight: HashMap<(ChangeRequestId, &'static str), OpId>,
    pending_comment: Option<PendingComment>,
    comment_retry_draft: Option<(ChangeRequestId, String)>,
    comment_retry_requires_refresh: Option<ChangeRequestId>,
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
        let mut repo_context = if demo {
            RepoContext {
                remote: "origin".into(),
                host: Some("github.com".into()),
                repository: Some("jack/quickdrop".into()),
                default_branch: Some("main".into()),
                ..RepoContext::default()
            }
        } else {
            repo::probe(&std::env::current_dir()?)
                .await
                .unwrap_or_default()
        };
        if repo_context.remote.is_empty() {
            repo_context.remote = "origin".into();
        }
        let providers = if demo {
            forge::demo::demo_providers().into_iter().collect()
        } else {
            providers(&config)
        };
        let mut app = Self {
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
            repo_context,
            project_git: None,
            palette_hits: vec![],
            activity: HashMap::new(),
            detail_resources: HashMap::new(),
            repository_info: HashMap::new(),
            repository_info_loading: std::collections::HashSet::new(),
            repository_info_failed: std::collections::HashSet::new(),
            config,
            demo,
            scope,
            events,
            providers,
            last_op: None,
            in_flight: HashMap::new(),
            pending_comment: None,
            comment_retry_draft: None,
            comment_retry_requires_refresh: None,
        };
        if demo {
            app.hydrate_demo_resources();
        }
        if !demo && let Some(root) = app.repo_context.root.clone() {
            let target = app.repo_context.default_branch.clone().unwrap_or_default();
            let remote = app.repo_context.remote.clone();
            let sender = app.events.clone();
            tokio::spawn(async move {
                let state = repo::branch_state(&root, &target, &remote).await;
                let result = if state.branch.is_empty() {
                    Err("could not determine the current Git branch".to_owned())
                } else {
                    Ok(state)
                };
                let _ = sender.send(AppEvent::ProjectGitLoaded(result));
            });
        }
        Ok(app)
    }
    #[cfg(test)]
    pub(crate) fn test_app() -> Self {
        let (events, _receiver) = mpsc::unbounded_channel();
        let config = Config::default();
        Self {
            requests: forge::demo::change_requests(),
            selected: 0,
            filter: String::new(),
            filtering: false,
            show_help: false,
            view: View::Dashboard,
            health: vec![],
            last_refresh: None,
            stale: false,
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
            repo_context: RepoContext {
                remote: "origin".into(),
                host: Some("github.com".into()),
                repository: Some("jack/quickdrop".into()),
                default_branch: Some("main".into()),
                ..RepoContext::default()
            },
            project_git: None,
            palette_hits: vec![],
            activity: HashMap::new(),
            detail_resources: HashMap::new(),
            repository_info: HashMap::new(),
            repository_info_loading: std::collections::HashSet::new(),
            repository_info_failed: std::collections::HashSet::new(),
            providers: forge::demo::demo_providers().into_iter().collect(),
            config,
            demo: true,
            scope: None,
            events,
            last_op: None,
            in_flight: HashMap::new(),
            pending_comment: None,
            comment_retry_draft: None,
            comment_retry_requires_refresh: None,
        }
    }
    pub fn toast(&self) -> Option<&str> {
        self.toast.as_deref()
    }
    pub fn comment_submission_pending(&self) -> bool {
        self.pending_comment.is_some()
    }
    pub fn set_toast(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.toast = (!message.trim().is_empty()).then_some(message);
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
    pub fn detail_resources_for(&self, id: &ChangeRequestId) -> Option<&DetailResources> {
        self.detail_resources.get(id)
    }
    fn hydrate_demo_resources(&mut self) {
        for request in self.requests.iter().cloned() {
            self.detail_resources.insert(
                request.id.clone(),
                DetailResources {
                    request: LoadState::Loaded(()),
                    comments: LoadState::Loaded(request.comments),
                    reviews: LoadState::Loaded(request.reviewers),
                    ci: LoadState::Loaded(request.pipelines),
                    comments_revision: 0,
                    refreshed_at: Some(Instant::now()),
                },
            );
        }
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
        if event.kind != KeyEventKind::Press {
            return false;
        }
        let key = event.code;
        if let Some(overlay) = self.overlay.take() {
            match overlay {
                Overlay::Composer {
                    mut body,
                    mut error,
                    retry_requires_refresh,
                    button_hits,
                } => {
                    let pending = self.pending_comment.is_some();
                    match key {
                        KeyCode::Esc if !pending && retry_requires_refresh => {
                            if let Some(request) = self.comment_retry_requires_refresh.clone() {
                                self.comment_retry_draft = Some((request, body));
                            }
                        }
                        KeyCode::Esc if !pending => {}
                        KeyCode::Esc => {
                            self.overlay = Some(Overlay::Composer {
                                body,
                                error,
                                retry_requires_refresh,
                                button_hits,
                            })
                        }
                        KeyCode::Backspace if !pending => {
                            body.pop();
                            if !retry_requires_refresh {
                                error = None;
                            }
                            self.overlay = Some(Overlay::Composer {
                                body,
                                error,
                                retry_requires_refresh,
                                button_hits,
                            });
                        }
                        KeyCode::Enter if event.modifiers.contains(KeyModifiers::CONTROL) => {
                            if pending {
                                self.overlay = Some(Overlay::Composer {
                                    body,
                                    error,
                                    retry_requires_refresh,
                                    button_hits,
                                });
                            } else if retry_requires_refresh {
                                self.set_toast("Refresh comments before retrying");
                                self.overlay = Some(Overlay::Composer {
                                    body,
                                    error,
                                    retry_requires_refresh,
                                    button_hits,
                                });
                            } else {
                                self.submit_comment(body);
                            }
                        }
                        KeyCode::Enter if !pending => {
                            body.push('\n');
                            if !retry_requires_refresh {
                                error = None;
                            }
                            self.overlay = Some(Overlay::Composer {
                                body,
                                error,
                                retry_requires_refresh,
                                button_hits,
                            });
                        }
                        KeyCode::Char(c) if !pending => {
                            body.push(c);
                            if !retry_requires_refresh {
                                error = None;
                            }
                            self.overlay = Some(Overlay::Composer {
                                body,
                                error,
                                retry_requires_refresh,
                                button_hits,
                            });
                        }
                        _ => {
                            self.overlay = Some(Overlay::Composer {
                                body,
                                error,
                                retry_requires_refresh,
                                button_hits,
                            })
                        }
                    }
                }
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
                        _ => self.open_comment_composer(),
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
                        selected = (selected + 1).min(self.palette_command_count(&query));
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
                        self.set_toast(
                            "Delete is capability-gated and not available in this build",
                        );
                    }
                    KeyCode::Esc | KeyCode::Enter => {}
                    _ => self.overlay = Some(Overlay::ConfirmDelete),
                },
                Overlay::ConfirmCi { action } => match key {
                    KeyCode::Enter | KeyCode::Esc => {}
                    KeyCode::Char('y') => self.start_ci_action(action),
                    _ => self.overlay = Some(Overlay::ConfirmCi { action }),
                },
                Overlay::Create(mut session) => {
                    let close = session.handle_key(self, key, event.modifiers);
                    if self.overlay.is_none() && !close {
                        self.overlay = Some(Overlay::Create(session));
                    }
                }
                Overlay::Edit(mut session) => {
                    let close = session.handle_key(self, key, event.modifiers);
                    if self.overlay.is_none() && !close {
                        self.overlay = Some(Overlay::Edit(session));
                    }
                }
                Overlay::Merge(mut session) => {
                    if key == KeyCode::Char('r') && session.preflight_error.is_some() {
                        session.preflight_error = None;
                        session.loading = true;
                        self.load_merge_preflight(session.id.clone());
                    } else {
                        let close = session.handle_key(self, key, event.modifiers);
                        if self.overlay.is_none() && !close {
                            self.overlay = Some(Overlay::Merge(session));
                        }
                    }
                }
                Overlay::Confirm(mut dialog) => match key {
                    KeyCode::Esc => {}
                    KeyCode::Left | KeyCode::Char('h') => dialog.selected = 0,
                    KeyCode::Right | KeyCode::Char('l') => dialog.selected = 1,
                    KeyCode::Enter if dialog.selected == 1 => self.confirm(dialog.action),
                    KeyCode::Enter => {}
                    _ => self.overlay = Some(Overlay::Confirm(dialog)),
                },
                Overlay::BranchCleanup(mut session) => match key {
                    KeyCode::Esc => {}
                    KeyCode::Up | KeyCode::Char('k') => {
                        session.selected = session.selected.saturating_sub(1)
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        session.selected =
                            (session.selected + 1).min(session.choices.len().saturating_sub(1))
                    }
                    KeyCode::Enter if session.selected == 0 => {}
                    KeyCode::Enter if !session.pending => self.start_branch_cleanup(session),
                    _ => self.overlay = Some(Overlay::BranchCleanup(session)),
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
    fn open_comment_composer(&mut self) {
        let request = self.request_for_view().map(|item| item.id.clone());
        let saved_draft = self.comment_retry_draft.take();
        let body = match (request.as_ref(), saved_draft) {
            (Some(request), Some((draft_request, body))) if *request == draft_request => body,
            (_, Some(draft)) => {
                self.comment_retry_draft = Some(draft);
                String::new()
            }
            _ => String::new(),
        };
        let retry_requires_refresh = request
            .as_ref()
            .is_some_and(|request| self.comment_retry_requires_refresh.as_ref() == Some(request));
        self.overlay = Some(Overlay::Composer {
            body,
            error: retry_requires_refresh
                .then(|| "Previous submission timed out; refresh comments before retrying".into()),
            retry_requires_refresh,
            button_hits: vec![],
        });
    }
    fn handle_dashboard_key(&mut self, key: KeyCode) -> bool {
        match key {
            KeyCode::Char('q') => return true,
            KeyCode::Char('n') => self.open_create(),
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
                self.open_comment_composer()
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
                self.set_toast("This forge has not advertised this write capability");
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
            KeyCode::Char('r') => self.hydrate_detail(id.clone(), true),
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
                self.open_comment_composer()
            }
            KeyCode::Char('e') => self.open_edit(),
            KeyCode::Char('M') => self.open_merge(),
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
                self.set_toast("This forge has not advertised this write capability");
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
            self.set_toast("No pipeline reported");
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
            self.set_toast("No provider is configured for this job");
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
                self.set_toast("No provider is configured for this CI action");
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
            self.set_toast(format!("{description} in progress"));
            return;
        }
        match action {
            CiAction::RetryJob(id) => {
                self.set_toast("Retry job requested");
                self.set_job_status(&id, PipelineStatus::Running);
            }
            CiAction::RetryPipeline(id) => {
                self.set_toast("Retry pipeline requested");
                self.set_pipeline_status(&id, PipelineStatus::Running);
            }
            CiAction::CancelJob(id) => {
                self.set_toast("Cancel job requested");
                self.set_job_status(&id, PipelineStatus::Cancelled);
            }
            CiAction::CancelPipeline(id) => {
                self.set_toast("Cancel pipeline requested");
                self.set_pipeline_status(&id, PipelineStatus::Cancelled);
            }
            CiAction::PlayJob(id) => {
                self.set_toast("Manual job started");
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
            self.load_repository_info(id.forge.clone(), id.repository.clone());
            self.hydrate_detail(id, false);
        }
    }
    fn hydrate_detail(&mut self, request: ChangeRequestId, force: bool) {
        if self.demo {
            if let Some(item) = self
                .requests
                .iter()
                .find(|item| item.id == request)
                .cloned()
            {
                let resources = self.detail_resources.entry(request).or_default();
                resources.request = LoadState::Loaded(());
                resources.comments = LoadState::Loaded(item.comments.clone());
                resources.reviews = LoadState::Loaded(item.reviewers.clone());
                resources.ci = LoadState::Loaded(item.pipelines.clone());
                resources.refreshed_at = Some(Instant::now());
            }
            return;
        }
        let stale = self
            .detail_resources
            .get(&request)
            .and_then(|resources| resources.refreshed_at)
            .is_none_or(|time| time.elapsed() >= std::time::Duration::from_secs(60));
        let Some(provider) = self.providers.get(&request.forge).cloned() else {
            let resources = self.detail_resources.entry(request).or_default();
            resources.request = LoadState::Failed("no provider is configured".into());
            resources.comments = LoadState::Failed("no provider is configured".into());
            resources.reviews = LoadState::Failed("no provider is configured".into());
            resources.ci = LoadState::Failed("no provider is configured".into());
            return;
        };
        let resources = self.detail_resources.entry(request.clone()).or_default();
        let load_request = begin_load(&mut resources.request, force, stale);
        let load_comments = begin_load(&mut resources.comments, force, stale);
        let load_reviews = begin_load(&mut resources.reviews, force, stale);
        let can_read_ci = provider.capabilities().ci_read;
        let load_ci = if can_read_ci {
            begin_load(&mut resources.ci, force, stale)
        } else {
            resources.ci = LoadState::Unsupported;
            false
        };
        let sender = self.events.clone();
        if load_request {
            let provider = provider.clone();
            let sender = sender.clone();
            let id = request.clone();
            tokio::spawn(async move {
                let result = provider.get_change_request(&id).await;
                let _ = sender.send(AppEvent::DetailRequestLoaded {
                    request: id,
                    result,
                });
            });
        }
        if load_comments {
            let provider = provider.clone();
            let sender = sender.clone();
            let id = request.clone();
            let revision = resources.comments_revision;
            tokio::spawn(async move {
                let result = provider.list_comments(&id).await;
                let _ = sender.send(AppEvent::CommentsLoaded {
                    request: id,
                    revision,
                    result,
                });
            });
        }
        if load_reviews {
            let provider = provider.clone();
            let sender = sender.clone();
            let id = request.clone();
            tokio::spawn(async move {
                let result = provider.list_reviews(&id).await;
                let _ = sender.send(AppEvent::ReviewsLoaded {
                    request: id,
                    result,
                });
            });
        }
        if load_ci {
            tokio::spawn(async move {
                let result = provider.list_pipelines(&request).await;
                let _ = sender.send(AppEvent::PipelinesLoaded {
                    request,
                    pipelines: result,
                });
            });
        }
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
        self.request_for_view()
            .and_then(|request| self.providers.get(&request.id.forge))
            .is_some_and(|provider| predicate(provider.capabilities()))
    }
    fn capabilities_for(&self, id: &ChangeRequestId) -> forge::ForgeCapabilities {
        let mut capabilities = self
            .providers
            .get(&id.forge)
            .map(|provider| provider.capabilities())
            .unwrap_or_default();
        if self.demo {
            return capabilities;
        }
        let key = (id.forge.clone(), id.repository.clone());
        if let Some(info) = self.repository_info.get(&key) {
            info.filter_capabilities(&mut capabilities);
        } else {
            capabilities.merge = false;
            capabilities.merge_commit = false;
            capabilities.squash_merge = false;
            capabilities.rebase_merge = false;
            capabilities.auto_merge = false;
        }
        capabilities
    }
    pub(crate) fn claim(&mut self, id: &ChangeRequestId, kind: &'static str) -> Option<OpId> {
        let key = (id.clone(), kind);
        if self.in_flight.contains_key(&key) {
            return None;
        }
        let op = OpId::next();
        self.in_flight.insert(key, op);
        self.last_op = Some(op);
        Some(op)
    }
    fn project_forge(&self) -> Option<(String, String, String, ChangeRequestKind)> {
        let host = self.repo_context.host.clone()?;
        let repository = self.repo_context.repository.clone()?;
        let forge = if self.demo {
            "github".to_owned()
        } else {
            self.config
                .forges
                .iter()
                .find(|forge| forge.host == host)
                .map(|forge| forge.name.clone())?
        };
        let kind = if self
            .config
            .forges
            .iter()
            .find(|config| config.name == forge)
            .is_some_and(|config| matches!(config.kind, ForgeKind::Gitlab))
        {
            ChangeRequestKind::MergeRequest
        } else {
            ChangeRequestKind::PullRequest
        };
        Some((forge, host, repository, kind))
    }
    pub(crate) fn can_create_current_project(&self) -> bool {
        self.project_forge().is_some_and(|(forge, _, _, _)| {
            self.providers
                .get(&forge)
                .is_some_and(|provider| provider.capabilities().create_change_request)
        })
    }
    fn open_create(&mut self) {
        let Some((forge, host, repository, kind)) = self.project_forge() else {
            self.set_toast("Create requires a configured Git repository and forge");
            return;
        };
        let Some(provider) = self.providers.get(&forge) else {
            self.set_toast("No provider is configured for this project");
            return;
        };
        let caps = provider.capabilities();
        if !caps.create_change_request {
            self.set_toast("This forge does not support creating change requests");
            return;
        }
        let mut context = self.repo_context.clone();
        context.host = Some(host);
        context.repository = Some(repository);
        let mut workflow = CreateWorkflow::new(forge, context, kind, caps, self.demo);
        workflow.start_preflight(self);
        if !self.demo {
            self.load_repository_info(workflow.forge.clone(), workflow.repository.clone());
        }
        self.overlay = Some(Overlay::Create(Box::new(workflow)));
    }
    fn load_repository_info(&mut self, forge: String, repository: String) {
        if self.demo {
            return;
        }
        let key = (forge.clone(), repository.clone());
        if self.repository_info.contains_key(&key)
            || !self.repository_info_loading.insert(key.clone())
        {
            return;
        }
        self.repository_info_failed.remove(&key);
        let Some(provider) = self.providers.get(&forge).cloned() else {
            self.repository_info_loading.remove(&key);
            self.repository_info_failed.insert(key);
            self.set_toast("No provider is configured for this repository");
            return;
        };
        let events = self.events.clone();
        tokio::spawn(async move {
            let result = provider.get_repository(&repository).await;
            let _ = events.send(AppEvent::RepositoryInfoLoaded {
                forge,
                repository,
                result,
            });
        });
    }
    fn open_edit(&mut self) {
        let Some(request) = self.detail_request().cloned() else {
            return;
        };
        let caps = self
            .providers
            .get(&request.id.forge)
            .map(|provider| provider.capabilities())
            .unwrap_or_default();
        self.overlay = Some(Overlay::Edit(EditSession::new(request.id, caps)));
    }
    fn open_edit_action(&mut self, action: EditAction) {
        let Some(request) = self.detail_request().cloned() else {
            return;
        };
        let caps = self
            .providers
            .get(&request.id.forge)
            .map(|provider| provider.capabilities())
            .unwrap_or_default();
        let mut session = EditSession::new(request.id, caps);
        session.dispatch(self, action);
        if self.overlay.is_none() {
            self.overlay = Some(Overlay::Edit(session));
        }
    }
    fn open_merge(&mut self) {
        let Some(request) = self.detail_request().cloned() else {
            return;
        };
        let mut caps = self
            .providers
            .get(&request.id.forge)
            .map(|provider| provider.capabilities())
            .unwrap_or_default();
        let repository_key = (request.id.forge.clone(), request.id.repository.clone());
        if let Some(info) = self.repository_info.get(&repository_key) {
            info.filter_capabilities(&mut caps);
        }
        if let Some(mut session) = MergeSession::build(&request, &caps) {
            if !self.demo && !self.repository_info.contains_key(&repository_key) {
                session.loading = true;
                self.repository_info_loading.insert(repository_key);
            }
            self.overlay = Some(Overlay::Merge(session));
            if !self.demo {
                self.load_merge_preflight(request.id.clone());
            }
        } else {
            self.set_toast("Merge is not supported or this request is not open");
        }
    }
    pub(crate) fn load_merge_preflight(&self, id: ChangeRequestId) {
        let Some(provider) = self.providers.get(&id.forge).cloned() else {
            return;
        };
        let events = self.events.clone();
        tokio::spawn(async move {
            let repository = id.repository.clone();
            let forge = id.forge.clone();
            let info = provider.get_repository(&repository).await;
            let _ = events.send(AppEvent::RepositoryInfoLoaded {
                forge,
                repository,
                result: info,
            });
            let result = async {
                let mut request = provider.get_change_request_for_merge(&id).await?;
                if provider.capabilities().ci_read {
                    let pipelines = provider.list_pipelines(&id).await?;
                    if !pipelines.is_empty() {
                        request.ci = summarize_ci(&pipelines);
                    }
                    request.pipelines = pipelines;
                }
                Ok(request)
            }
            .await;
            let _ = events.send(AppEvent::MergePreflightLoaded { id, result });
        });
    }
    pub(crate) fn spawn_preflight(
        &self,
        op: OpId,
        root: std::path::PathBuf,
        remote: String,
        target: String,
        kind: ChangeRequestKind,
    ) {
        let sender = self.events.clone();
        tokio::spawn(async move {
            let mut state = repo::branch_state(&root, &target, &remote).await;
            if state.branch.is_empty() {
                let _ = sender.send(AppEvent::GitPreflightCompleted {
                    op,
                    result: Err("could not determine the current Git branch".into()),
                });
                return;
            }
            let remote_exists =
                match repo::remote_branch_exists(&root, &remote, &state.branch).await {
                    Ok(exists) => exists,
                    Err(PushError::TimedOut) => {
                        let _ = sender.send(AppEvent::GitPreflightCompleted {
                            op,
                            result: Err("remote branch check timed out".into()),
                        });
                        return;
                    }
                    Err(PushError::Failed { .. }) => {
                        let _ = sender.send(AppEvent::GitPreflightCompleted {
                            op,
                            result: Err(
                                "could not verify the remote branch; check the configured remote"
                                    .into(),
                            ),
                        });
                        return;
                    }
                };
            let subjects = repo::commit_subjects(&root, &target, &remote, 5).await;
            let template = repo::find_template(&root, kind);
            state.remote_branch_exists = Some(remote_exists);
            if !remote_exists {
                state.unpushed = state.ahead;
            }
            let _ = sender.send(AppEvent::GitPreflightCompleted {
                op,
                result: Ok(GitPreflight {
                    state,
                    remote_exists: Some(remote_exists),
                    template,
                    subjects,
                }),
            });
        });
    }
    pub(crate) fn spawn_target_branches(
        &self,
        token: OpId,
        root: std::path::PathBuf,
        remote: String,
        source: String,
    ) {
        let sender = self.events.clone();
        tokio::spawn(async move {
            let branches = repo::branches(&root, &remote)
                .await
                .into_iter()
                .filter(|branch| branch != &source)
                .collect();
            let _ = sender.send(AppEvent::GitBranchesLoaded { token, branches });
        });
    }
    pub(crate) fn spawn_picker_search(
        &self,
        kind: PickerKind,
        token: OpId,
        forge: &str,
        repository: &str,
        query: &str,
    ) {
        let forge = forge.to_owned();
        let repository = repository.to_owned();
        let query = query.to_owned();
        let demo = self.demo;
        let provider = self.providers.get(&forge).cloned();
        let sender = self.events.clone();
        tokio::spawn(async move {
            let result = if demo {
                let q = query.to_lowercase();
                Ok(forge::demo::picker_items(kind)
                    .into_iter()
                    .filter(|item| {
                        q.is_empty()
                            || item.id.to_lowercase().contains(&q)
                            || item.label.to_lowercase().contains(&q)
                    })
                    .collect())
            } else if let Some(provider) = provider {
                match kind {
                    PickerKind::Reviewer => provider
                        .search_reviewers(
                            &ChangeRequestId {
                                forge: forge.clone(),
                                repository: repository.clone(),
                                number: 0,
                            },
                            &query,
                        )
                        .await
                        .map(|people| {
                            people
                                .into_iter()
                                .map(|person| {
                                    let id = person.login.clone();
                                    let label = person.display_name().to_owned();
                                    PickerItem {
                                        id,
                                        label,
                                        detail: person.id.map(|id| id.to_string()),
                                    }
                                })
                                .collect()
                        }),
                    PickerKind::Assignee => provider
                        .search_assignees(&repository, &query)
                        .await
                        .map(|people| {
                            people
                                .into_iter()
                                .map(|person| {
                                    let id = person.login.clone();
                                    let label = person.display_name().to_owned();
                                    PickerItem {
                                        id,
                                        label,
                                        detail: person.id.map(|id| id.to_string()),
                                    }
                                })
                                .collect()
                        }),
                    PickerKind::Label => provider.list_labels(&repository).await.map(|labels| {
                        labels
                            .into_iter()
                            .map(|label| PickerItem {
                                id: label.name.clone(),
                                label: label.name,
                                detail: label.color,
                            })
                            .collect()
                    }),
                    PickerKind::Milestone => {
                        provider
                            .list_milestones(&repository)
                            .await
                            .map(|milestones| {
                                milestones
                                    .into_iter()
                                    .map(|milestone| PickerItem::simple(milestone.name))
                                    .collect()
                            })
                    }
                    PickerKind::TargetBranch => Err(forge::ForgeError::Unsupported),
                }
            } else {
                Err(forge::ForgeError::NotFound)
            };
            let _ = sender.send(AppEvent::PickerLoaded { token, result });
        });
    }
    pub(crate) fn next_demo_number(&self, forge: &str, repository: &str) -> u64 {
        self.requests
            .iter()
            .filter(|request| request.id.forge == forge && request.id.repository == repository)
            .map(|request| request.id.number)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
    }
    pub(crate) fn start_update(
        &mut self,
        id: ChangeRequestId,
        patch: forge::RequestPatch,
        action: LifecycleAction,
    ) {
        let permitted = self.providers.get(&id.forge).is_some_and(|provider| {
            let caps = provider.capabilities();
            match action {
                LifecycleAction::Title => caps.edit_title,
                LifecycleAction::Body => caps.edit_description,
                LifecycleAction::Draft | LifecycleAction::Ready => caps.draft_transition,
                LifecycleAction::Close => caps.close,
                LifecycleAction::Reopen => caps.reopen,
            }
        });
        if !permitted {
            self.set_toast("This forge has not advertised this write capability");
            return;
        }
        let Some(op) = self.claim(&id, "update") else {
            return;
        };
        self.set_toast(match action {
            LifecycleAction::Title => "Updating title",
            LifecycleAction::Body => "Updating description",
            LifecycleAction::Draft => "Marking draft",
            LifecycleAction::Ready => "Marking ready for review",
            LifecycleAction::Close => "Closing request",
            LifecycleAction::Reopen => "Reopening request",
        });
        let sender = self.events.clone();
        if self.demo {
            let result = self
                .requests
                .iter()
                .find(|request| request.id == id)
                .cloned()
                .map(|mut request| {
                    if let Some(title) = patch.title.clone() {
                        request.title = title;
                    }
                    if let Some(body) = patch.body.clone() {
                        request.body = Some(body);
                    }
                    if let Some(draft) = patch.draft {
                        request.draft = draft;
                    }
                    if let Some(state) = patch.state {
                        request.state = state;
                    }
                    request.updated_at = Utc::now();
                    request
                })
                .ok_or(forge::ForgeError::NotFound);
            tokio::spawn(async move {
                let _ = sender.send(AppEvent::RequestWriteCompleted {
                    id,
                    op,
                    action,
                    result,
                });
            });
            return;
        }
        let Some(provider) = self.providers.get(&id.forge).cloned() else {
            self.in_flight.remove(&(id.clone(), "update"));
            self.set_toast("No provider is configured for this request");
            return;
        };
        tokio::spawn(async move {
            let result = provider.update_change_request(&id, &patch).await;
            let _ = sender.send(AppEvent::RequestWriteCompleted {
                id,
                op,
                action,
                result,
            });
        });
    }
    pub(crate) fn start_labels(&mut self, id: ChangeRequestId, labels: Vec<String>) {
        self.start_metadata(id, MetaKind::Labels, labels, vec![], None);
    }
    pub(crate) fn start_assignees(&mut self, id: ChangeRequestId, assignees: Vec<String>) {
        self.start_metadata(id, MetaKind::Assignees, assignees, vec![], None);
    }
    pub(crate) fn start_reviewers(
        &mut self,
        id: ChangeRequestId,
        add: Vec<Person>,
        remove: Vec<String>,
    ) {
        self.start_metadata(id, MetaKind::Reviewers, vec![], add, Some(remove));
    }
    pub(crate) fn start_milestone(&mut self, id: ChangeRequestId, milestone: Option<String>) {
        self.start_metadata(
            id,
            MetaKind::Milestone,
            vec![],
            vec![],
            Some(vec![milestone.unwrap_or_default()]),
        );
    }
    fn start_metadata(
        &mut self,
        id: ChangeRequestId,
        kind: MetaKind,
        names: Vec<String>,
        people: Vec<Person>,
        optional: Option<Vec<String>>,
    ) {
        let cap_ok = self.providers.get(&id.forge).is_some_and(|provider| {
            let caps = provider.capabilities();
            match kind {
                MetaKind::Labels => caps.labels,
                MetaKind::Reviewers => caps.request_reviewers,
                MetaKind::Assignees => caps.assignees,
                MetaKind::Milestone => caps.milestone,
            }
        });
        if !cap_ok {
            self.set_toast("This forge has not advertised this write capability");
            return;
        }
        let key = match kind {
            MetaKind::Labels => "labels",
            MetaKind::Reviewers => "reviewers",
            MetaKind::Assignees => "assignees",
            MetaKind::Milestone => "milestone",
        };
        let Some(op) = self.claim(&id, key) else {
            return;
        };
        self.set_toast("Saving change request metadata");
        let sender = self.events.clone();
        if self.demo {
            let result = self
                .requests
                .iter()
                .find(|request| request.id == id)
                .cloned()
                .map(|request| {
                    let mut payload = edit::MetaPayload::default();
                    match kind {
                        MetaKind::Labels => {
                            payload.labels = names
                                .iter()
                                .map(|name| Label::named(name.clone()))
                                .collect();
                            payload.labels_set = true;
                        }
                        MetaKind::Assignees => {
                            payload.assignees = names
                                .iter()
                                .map(|login| Person::named(login.clone()))
                                .collect();
                            payload.assignees_set = true;
                        }
                        MetaKind::Milestone => {
                            payload.milestone = optional
                                .as_ref()
                                .and_then(|values| values.first())
                                .filter(|value| !value.is_empty())
                                .cloned();
                            payload.milestone_set = true;
                        }
                        MetaKind::Reviewers => {
                            let current = request
                                .reviewers
                                .iter()
                                .filter(|reviewer| reviewer.state == ReviewState::Requested)
                                .map(|reviewer| reviewer.person.login.clone())
                                .collect::<Vec<_>>();
                            let removes = optional.unwrap_or_default();
                            let mut reviewers = request
                                .reviewers
                                .into_iter()
                                .filter(|reviewer| {
                                    reviewer.state != ReviewState::Requested
                                        || (!removes.contains(&reviewer.person.login)
                                            && (people.is_empty()
                                                || !current.contains(&reviewer.person.login)))
                                })
                                .collect::<Vec<_>>();
                            reviewers.extend(people.iter().map(|person| Reviewer {
                                person: person.clone(),
                                state: ReviewState::Requested,
                            }));
                            payload.reviewers = reviewers;
                            payload.reviewers_set = true;
                        }
                    }
                    payload
                })
                .ok_or(forge::ForgeError::NotFound);
            tokio::spawn(async move {
                let _ = sender.send(AppEvent::MetadataWriteCompleted {
                    id,
                    op,
                    kind,
                    result,
                });
            });
            return;
        }
        let Some(provider) = self.providers.get(&id.forge).cloned() else {
            self.in_flight.remove(&(id.clone(), key));
            self.set_toast("No provider is configured for this request");
            return;
        };
        tokio::spawn(async move {
            let result = match kind {
                MetaKind::Labels => {
                    provider
                        .set_labels(&id, &names)
                        .await
                        .map(|labels| edit::MetaPayload {
                            labels,
                            labels_set: true,
                            ..edit::MetaPayload::default()
                        })
                }
                MetaKind::Assignees => {
                    provider
                        .set_assignees(&id, &names)
                        .await
                        .map(|assignees| edit::MetaPayload {
                            assignees,
                            assignees_set: true,
                            ..edit::MetaPayload::default()
                        })
                }
                MetaKind::Milestone => {
                    let milestone = optional
                        .as_ref()
                        .and_then(|values| values.first())
                        .filter(|value| !value.is_empty())
                        .map(String::as_str);
                    provider
                        .set_milestone(&id, milestone)
                        .await
                        .map(|milestone| edit::MetaPayload {
                            milestone,
                            milestone_set: true,
                            ..edit::MetaPayload::default()
                        })
                }
                MetaKind::Reviewers => {
                    let removes = optional.unwrap_or_default();
                    let mut result = Ok(());
                    for reviewer in removes {
                        if let Err(error) = provider.remove_reviewer(&id, &reviewer).await {
                            result = Err(error);
                            break;
                        }
                    }
                    if result.is_ok() {
                        for person in &people {
                            if let Err(error) = provider.request_reviewer(&id, person).await {
                                result = Err(error);
                                break;
                            }
                        }
                    }
                    match result {
                        Ok(()) => provider.get_change_request(&id).await.map(|request| {
                            edit::MetaPayload {
                                reviewers: request.reviewers,
                                reviewers_set: true,
                                ..edit::MetaPayload::default()
                            }
                        }),
                        Err(error) => Err(error),
                    }
                }
            };
            let _ = sender.send(AppEvent::MetadataWriteCompleted {
                id,
                op,
                kind,
                result,
            });
        });
    }
    pub(crate) fn start_merge(&mut self, id: ChangeRequestId, strategy: MergeStrategy, op: OpId) {
        if self.in_flight.contains_key(&(id.clone(), "merge")) {
            return;
        }
        self.in_flight.insert((id.clone(), "merge"), op);
        let sender = self.events.clone();
        if self.demo {
            let result = Ok(MergeOutcome {
                sha: Some(format!("demo-merge-{}", id.number)),
                message: Some(format!("Merged with {}", strategy.label())),
            });
            tokio::spawn(async move {
                let _ = sender.send(AppEvent::MergeCompleted { id, op, result });
            });
            return;
        }
        let Some(provider) = self.providers.get(&id.forge).cloned() else {
            self.in_flight.remove(&(id.clone(), "merge"));
            self.set_toast("No provider is configured for this request");
            return;
        };
        tokio::spawn(async move {
            let result = provider.merge_change_request(&id, strategy).await;
            let _ = sender.send(AppEvent::MergeCompleted { id, op, result });
        });
    }
    fn start_auto_merge(&mut self, id: ChangeRequestId, enabled: bool) {
        let Some(provider) = self.providers.get(&id.forge).cloned() else {
            self.set_toast("No provider is configured for this request");
            return;
        };
        let caps = self.capabilities_for(&id);
        if !caps.auto_merge {
            self.set_toast("Auto-merge is not supported by this forge");
            return;
        }
        let strategy = [
            (MergeStrategy::Squash, caps.squash_merge),
            (MergeStrategy::MergeCommit, caps.merge_commit),
            (MergeStrategy::Rebase, caps.rebase_merge),
        ]
        .into_iter()
        .find_map(|(strategy, supported)| supported.then_some(strategy));
        let Some(strategy) = strategy else {
            self.set_toast("No supported strategy is available for auto-merge");
            return;
        };
        self.set_toast(if enabled {
            "Enabling auto-merge"
        } else {
            "Disabling auto-merge"
        });
        let Some(op) = self.claim(&id, "auto-merge") else {
            return;
        };
        let sender = self.events.clone();
        if self.demo {
            tokio::spawn(async move {
                let _ = sender.send(AppEvent::AutoMergeCompleted {
                    id,
                    op,
                    enabled,
                    result: Ok(enabled),
                });
            });
            return;
        }
        tokio::spawn(async move {
            let result = provider.set_auto_merge(&id, enabled, strategy).await;
            let _ = sender.send(AppEvent::AutoMergeCompleted {
                id,
                op,
                enabled,
                result,
            });
        });
    }
    fn confirm(&mut self, action: ConfirmAction) {
        self.overlay = None;
        match action {
            ConfirmAction::CloseRequest(id) => self.start_update(
                id,
                forge::RequestPatch {
                    state: Some(RequestState::Closed),
                    ..forge::RequestPatch::default()
                },
                LifecycleAction::Close,
            ),
            ConfirmAction::DeleteRemoteBranch { id, branch } => {
                self.start_branch_delete(id, branch)
            }
        }
    }
    fn start_branch_delete(&mut self, id: ChangeRequestId, branch: String) {
        let Some(provider) = self.providers.get(&id.forge).cloned() else {
            self.set_toast("No provider is configured for this request");
            return;
        };
        if !provider.capabilities().delete_source_branch {
            self.set_toast("Source branch deletion is not supported");
            return;
        }
        let sender = self.events.clone();
        tokio::spawn(async move {
            let result = provider.delete_branch(&id.repository, &branch).await;
            let _ = sender.send(AppEvent::BranchCleanupCompleted {
                id,
                message: "Remote source branch deleted".into(),
                result: result.map_err(|error| error_summary(&error)),
            });
        });
    }
    fn start_branch_cleanup(&mut self, mut session: BranchCleanupSession) {
        let Some(choice) = session.choices.get(session.selected).copied() else {
            self.overlay = None;
            return;
        };
        if choice == BranchCleanupChoice::KeepBranches {
            self.overlay = None;
            return;
        }
        session.pending = true;
        self.overlay = Some(Overlay::BranchCleanup(session.clone()));
        self.set_toast("Deleting source branch");
        let id = session.id.clone();
        let branch = session.branch.clone();
        let root = session.root.clone();
        let provider = self.providers.get(&id.forge).cloned();
        let events = self.events.clone();
        tokio::spawn(async move {
            let result = async {
                if matches!(
                    choice,
                    BranchCleanupChoice::DeleteRemote | BranchCleanupChoice::DeleteBoth
                ) {
                    let provider = provider
                        .as_ref()
                        .ok_or_else(|| "provider unavailable".to_owned())?;
                    provider
                        .delete_branch(&id.repository, &branch)
                        .await
                        .map_err(|error| error_summary(&error))?;
                }
                if matches!(
                    choice,
                    BranchCleanupChoice::DeleteLocal | BranchCleanupChoice::DeleteBoth
                ) {
                    let root = root
                        .as_ref()
                        .ok_or_else(|| "local repository is unavailable".to_owned())?;
                    repo::delete_local_branch(root, &branch)
                        .await
                        .map_err(|error| -> String {
                            match error {
                                PushError::TimedOut => "git branch delete timed out".into(),
                                PushError::Failed { .. } => {
                                    "git could not delete the local branch safely".into()
                                }
                            }
                        })?;
                }
                Ok(())
            }
            .await;
            let message = match choice {
                BranchCleanupChoice::DeleteRemote => "Remote source branch deleted",
                BranchCleanupChoice::DeleteLocal => "Local source branch deleted",
                BranchCleanupChoice::DeleteBoth => "Local and remote source branches deleted",
                BranchCleanupChoice::KeepBranches => "Branches kept",
            }
            .to_owned();
            let _ = events.send(AppEvent::BranchCleanupCompleted {
                id,
                message,
                result,
            });
        });
    }
    fn load_request(&self, id: ChangeRequestId) {
        if self.demo {
            return;
        }
        if let Some(provider) = self.providers.get(&id.forge).cloned() {
            let events = self.events.clone();
            tokio::spawn(async move {
                let result = provider.get_change_request(&id).await;
                let _ = events.send(AppEvent::TargetedRequestLoaded { id, result });
            });
        }
    }
    pub fn apply_git_preflight(&mut self, op: OpId, result: Result<GitPreflight, String>) {
        let Some(Overlay::Create(session)) = &mut self.overlay else {
            return;
        };
        if session.preflight_op != Some(op) {
            return;
        }
        session.preflight_loading = false;
        match result {
            Ok(preflight) => session.apply_preflight(
                preflight.state,
                preflight.remote_exists,
                preflight.template,
                preflight.subjects,
            ),
            Err(error) => {
                session.preflight_error = Some(error);
                session.set_button(vec![create::Button::Cancel]);
            }
        }
    }
    pub fn apply_project_git(&mut self, result: Result<BranchState, String>) {
        match result {
            Ok(state) => self.project_git = Some(state),
            Err(error) => self.set_toast(format!("Git status unavailable: {error}")),
        }
    }
    pub fn apply_repository_info(
        &mut self,
        forge: String,
        repository: String,
        result: Result<forge::RepositoryInfo, forge::ForgeError>,
    ) {
        match result {
            Ok(info) => {
                let key = (forge.clone(), repository.clone());
                self.repository_info_loading.remove(&key);
                self.repository_info_failed.remove(&key);
                if self.repository_info.get(&key) == Some(&info) {
                    return;
                }
                self.repository_info.insert(key, info.clone());
                if self.repo_context.repository.as_deref() == Some(repository.as_str()) {
                    self.repo_context.default_branch = info
                        .default_branch
                        .clone()
                        .or(self.repo_context.default_branch.clone());
                }
                let mut restart_create = false;
                if let Some(Overlay::Create(session)) = &mut self.overlay
                    && session.forge == forge
                    && session.repository == repository
                    && session.target.is_empty()
                    && let Some(default_branch) = info.default_branch
                {
                    session.target = default_branch;
                    restart_create = true;
                }
                if restart_create && let Some(Overlay::Create(mut session)) = self.overlay.take() {
                    session.start_preflight(self);
                    self.overlay = Some(Overlay::Create(session));
                }
                let pending_merge = self.overlay.as_ref().and_then(|overlay| match overlay {
                    Overlay::Merge(session)
                        if session.id.forge == forge && session.id.repository == repository =>
                    {
                        Some((
                            session.id.clone(),
                            session.selected_strategy(),
                            session.loading,
                        ))
                    }
                    _ => None,
                });
                if let Some((id, strategy, loading)) = pending_merge
                    && let Some(request) = self.requests.iter().find(|request| request.id == id)
                    && let Some(mut session) =
                        MergeSession::build(request, &self.capabilities_for(&id))
                {
                    if let Some(index) = session
                        .strategies
                        .iter()
                        .position(|candidate| *candidate == strategy)
                    {
                        session.strategy = index;
                    }
                    session.loading = loading;
                    self.overlay = Some(Overlay::Merge(session));
                }
            }
            Err(error) => {
                let key = (forge.clone(), repository.clone());
                self.repository_info_loading.remove(&key);
                self.repository_info_failed.insert(key);
                if matches!(self.overlay, Some(Overlay::Create(_))) {
                    self.set_toast(format!(
                        "Repository settings unavailable: {}",
                        error_summary(&error)
                    ));
                }
                if let Some(Overlay::Merge(session)) = &mut self.overlay
                    && session.id.forge == forge
                    && session.id.repository == repository
                {
                    session.preflight_error = Some(format!(
                        "Repository merge policy unavailable: {}",
                        error_summary(&error)
                    ));
                }
            }
        }
    }
    pub fn apply_merge_preflight(
        &mut self,
        id: ChangeRequestId,
        result: Result<ChangeRequest, forge::ForgeError>,
    ) {
        let Some(current) = self.overlay.as_ref().and_then(|overlay| match overlay {
            Overlay::Merge(session) if session.id == id => Some(session.clone()),
            _ => None,
        }) else {
            return;
        };
        match result {
            Ok(request) => {
                let caps = self.capabilities_for(&id);
                if let Some(mut refreshed) = MergeSession::build(&request, &caps) {
                    if let Some(index) = refreshed
                        .strategies
                        .iter()
                        .position(|strategy| *strategy == current.selected_strategy())
                    {
                        refreshed.strategy = index;
                    }
                    self.reconcile_request(request);
                    self.overlay = Some(Overlay::Merge(refreshed));
                } else {
                    self.overlay = None;
                    self.set_toast(
                        if self
                            .repository_info_failed
                            .contains(&(id.forge.clone(), id.repository.clone()))
                        {
                            "Repository merge policy could not be verified"
                        } else {
                            "This request is no longer open or the repository allows no merge strategy"
                        },
                    );
                }
            }
            Err(error) => {
                if let Some(Overlay::Merge(session)) = &mut self.overlay {
                    session.loading = false;
                    session.preflight_error = Some(error_summary(&error));
                }
            }
        }
    }
    pub fn apply_git_branches(&mut self, token: OpId, branches: Vec<String>) {
        let Some(Overlay::Create(session)) = &mut self.overlay else {
            return;
        };
        if let Some(create::CreateEditor::Target(picker)) = &mut session.editor {
            picker.apply_items(
                token,
                branches.into_iter().map(PickerItem::simple).collect(),
            );
        }
    }
    pub fn apply_picker(
        &mut self,
        token: OpId,
        result: Result<Vec<PickerItem>, forge::ForgeError>,
    ) {
        let apply =
            |picker: &mut PickerSession,
             token,
             result: Result<Vec<PickerItem>, forge::ForgeError>| match result {
                Ok(items) => picker.apply_items(token, items),
                Err(error) => picker.failed(token, error_summary(&error)),
            };
        match self.overlay.as_mut() {
            Some(Overlay::Create(session)) => {
                if let Some(
                    create::CreateEditor::Target(picker)
                    | create::CreateEditor::Reviewers(picker)
                    | create::CreateEditor::Labels(picker)
                    | create::CreateEditor::Assignees(picker)
                    | create::CreateEditor::Milestone(picker),
                ) = session.editor.as_mut()
                {
                    apply(picker, token, result)
                }
            }
            Some(Overlay::Edit(session)) => {
                if let Some(edit::EditField::Picker(picker)) = session.active.as_mut() {
                    apply(picker, token, result);
                }
            }
            _ => {}
        }
    }
    pub fn apply_push(&mut self, op: OpId, branch: String, result: Result<(), PushError>) {
        let Some(Overlay::Create(session)) = &mut self.overlay else {
            return;
        };
        if session.push_op != Some(op) {
            return;
        }
        session.apply_push(op, branch, result);
    }
    pub fn apply_create(
        &mut self,
        op: OpId,
        result: Result<forge::CreateResult, forge::ForgeError>,
    ) {
        let metadata_warnings = result
            .as_ref()
            .ok()
            .map(|created| created.metadata_warnings.clone())
            .unwrap_or_default();
        let (created, failure) = match &mut self.overlay {
            Some(Overlay::Create(session)) => {
                let created = session.apply_submit(op, result.map(|created| created.request));
                let failure = session.failure_message();
                (created, failure)
            }
            _ => return,
        };
        let Some(created) = created else {
            if let Some(failure) = failure {
                self.set_toast(format!("Create failed: {failure}"));
            }
            return;
        };
        let id = created.id.clone();
        self.reconcile_request(created.clone());
        self.overlay = None;
        self.view = View::ChangeRequestDetail(id.clone());
        self.set_toast(if metadata_warnings.is_empty() {
            format!("Created {}", id.display(created.kind))
        } else {
            format!(
                "Created {}; metadata needs attention: {}",
                id.display(created.kind),
                metadata_warnings.join("; ")
            )
        });
        self.load_request(id);
    }
    fn reconcile_request(&mut self, request: ChangeRequest) {
        if let Some(current) = self
            .requests
            .iter_mut()
            .find(|current| current.id == request.id)
        {
            *current = request;
        } else {
            self.requests.push(request);
        }
    }
    pub fn apply_request_write(
        &mut self,
        id: ChangeRequestId,
        op: OpId,
        action: LifecycleAction,
        result: Result<ChangeRequest, forge::ForgeError>,
    ) {
        if self.in_flight.get(&(id.clone(), "update")) != Some(&op) {
            return;
        }
        self.in_flight.remove(&(id.clone(), "update"));
        match result {
            Ok(updated) => {
                if let Some(current) = self.requests.iter_mut().find(|request| request.id == id) {
                    match action {
                        LifecycleAction::Title => current.title = updated.title,
                        LifecycleAction::Body => current.body = updated.body,
                        LifecycleAction::Draft | LifecycleAction::Ready => {
                            current.draft = updated.draft
                        }
                        LifecycleAction::Close | LifecycleAction::Reopen => {
                            current.state = updated.state
                        }
                    }
                    current.updated_at = updated.updated_at;
                }
                self.finish_edit_write(op, true);
                self.set_toast(match action {
                    LifecycleAction::Title => "Title updated",
                    LifecycleAction::Body => "Description updated",
                    LifecycleAction::Draft => "Marked as draft",
                    LifecycleAction::Ready => "Marked ready for review",
                    LifecycleAction::Close => "Request closed",
                    LifecycleAction::Reopen => "Request reopened",
                });
                self.load_request(id);
            }
            Err(error) => {
                self.finish_edit_write(op, false);
                self.set_toast(format!("Update failed: {}", error_summary(&error)));
            }
        }
    }
    fn finish_edit_write(&mut self, op: OpId, success: bool) {
        if let Some(Overlay::Edit(session)) = &mut self.overlay
            && session
                .pending
                .as_ref()
                .is_some_and(|(pending, _)| *pending == op)
        {
            session.pending = None;
            if success {
                session.active = None;
            }
        }
    }
    pub fn apply_metadata_write(
        &mut self,
        id: ChangeRequestId,
        op: OpId,
        kind: MetaKind,
        result: Result<edit::MetaPayload, forge::ForgeError>,
    ) {
        let key = match kind {
            MetaKind::Labels => "labels",
            MetaKind::Reviewers => "reviewers",
            MetaKind::Assignees => "assignees",
            MetaKind::Milestone => "milestone",
        };
        if self.in_flight.get(&(id.clone(), key)) != Some(&op) {
            return;
        }
        self.in_flight.remove(&(id.clone(), key));
        match result {
            Ok(payload) => {
                if let Some(request) = self.requests.iter_mut().find(|request| request.id == id) {
                    edit::apply_metadata_payload(request, &payload);
                }
                self.finish_edit_write(op, true);
                self.set_toast("Change request metadata updated");
                self.load_request(id);
            }
            Err(error) => {
                self.finish_edit_write(op, false);
                self.set_toast(format!("Metadata update failed: {}", error_summary(&error)));
            }
        }
    }
    pub fn apply_merge(
        &mut self,
        id: ChangeRequestId,
        op: OpId,
        result: Result<MergeOutcome, forge::ForgeError>,
    ) {
        if self.in_flight.get(&(id.clone(), "merge")) != Some(&op) {
            return;
        }
        self.in_flight.remove(&(id.clone(), "merge"));
        match result {
            Ok(outcome) => {
                if let Some(Overlay::Merge(session)) = &mut self.overlay {
                    session.apply(op, Ok(outcome.clone()));
                }
                if let Some(request) = self.requests.iter_mut().find(|request| request.id == id) {
                    request.state = RequestState::Merged;
                    request.merged_sha = outcome.sha.clone();
                }
                self.activity
                    .entry(id.clone())
                    .or_default()
                    .push(outcome.message.unwrap_or_else(|| "Merged".into()));
                let cleanup =
                    self.requests
                        .iter()
                        .find(|request| request.id == id)
                        .map(|request| {
                            let remote = self.providers.get(&id.forge).is_some_and(|provider| {
                                provider.capabilities().delete_source_branch
                            });
                            let same_repo = self.repo_context.repository.as_deref()
                                == Some(id.repository.as_str());
                            let local_root =
                                same_repo.then(|| self.repo_context.root.clone()).flatten();
                            let mut choices = vec![BranchCleanupChoice::KeepBranches];
                            if remote {
                                choices.push(BranchCleanupChoice::DeleteRemote);
                            }
                            if local_root.is_some() {
                                choices.push(BranchCleanupChoice::DeleteLocal);
                            }
                            if remote && local_root.is_some() {
                                choices.push(BranchCleanupChoice::DeleteBoth);
                            }
                            BranchCleanupSession {
                                id: id.clone(),
                                kind: request.kind,
                                branch: request.source_branch.clone(),
                                root: local_root,
                                remote: self.repo_context.remote.clone(),
                                choices,
                                selected: 0,
                                pending: false,
                                button_hits: vec![],
                            }
                        });
                self.set_toast("Request merged");
                self.load_request(id.clone());
                if let Some(cleanup) = cleanup {
                    self.overlay = Some(Overlay::BranchCleanup(cleanup));
                } else {
                    self.overlay = None;
                }
            }
            Err(error) => {
                let message = error_summary(&error);
                if let Some(Overlay::Merge(session)) = &mut self.overlay {
                    session.apply(op, Err(message.clone()));
                }
                self.set_toast(format!("Merge failed: {message}"));
            }
        }
    }
    pub fn apply_auto_merge(
        &mut self,
        id: ChangeRequestId,
        op: OpId,
        enabled: bool,
        result: Result<bool, forge::ForgeError>,
    ) {
        if self.in_flight.get(&(id.clone(), "auto-merge")) != Some(&op) {
            return;
        }
        self.in_flight.remove(&(id.clone(), "auto-merge"));
        match result {
            Ok(value) => {
                if let Some(request) = self.requests.iter_mut().find(|request| request.id == id) {
                    request.auto_merge = value;
                }
                self.set_toast(if enabled {
                    "Auto-merge enabled"
                } else {
                    "Auto-merge disabled"
                });
                self.load_request(id);
            }
            Err(error) => {
                self.set_toast(format!("Auto-merge failed: {}", error_summary(&error)));
            }
        }
    }
    pub fn apply_targeted_request(
        &mut self,
        id: ChangeRequestId,
        result: Result<ChangeRequest, forge::ForgeError>,
    ) {
        match result {
            Ok(request) => self.reconcile_request(request),
            Err(error) => self.set_toast(format!("Refresh failed: {}", error_summary(&error))),
        }
        if self.view == View::ChangeRequestDetail(id.clone()) {
            self.hydrate_detail(id, true);
        }
    }
    pub fn apply_branch_cleanup(
        &mut self,
        id: ChangeRequestId,
        message: String,
        result: Result<(), String>,
    ) {
        match result {
            Ok(()) => {
                self.activity.entry(id).or_default().push(message.clone());
                self.set_toast(message);
            }
            Err(error) => self.set_toast(format!("Branch cleanup failed: {error}")),
        }
        if matches!(self.overlay, Some(Overlay::BranchCleanup(_))) {
            self.overlay = None;
        }
    }
    fn submit_comment(&mut self, body: String) {
        if self.pending_comment.is_some() {
            return;
        }
        if body.trim().is_empty() {
            self.set_toast("Comment is empty");
            self.set_composer_error(body, "Comment is empty".into(), false);
            return;
        }
        let Some(request) = self.request_for_view().map(|item| item.id.clone()) else {
            return;
        };
        if self.comment_retry_requires_refresh.as_ref() == Some(&request) {
            self.set_toast("Refresh comments before retrying");
            self.set_composer_error(
                body,
                "Previous submission timed out; refresh comments before retrying".into(),
                true,
            );
            return;
        }
        if self
            .comment_retry_draft
            .as_ref()
            .is_some_and(|(draft_request, _)| draft_request == &request)
        {
            self.comment_retry_draft = None;
        }
        let correlation_id = OpId::next();
        if self.demo {
            let comment = Comment {
                id: format!("demo-{}", correlation_id.0),
                author: Person {
                    login: "jack".into(),
                    name: Some("Jack".into()),
                    id: None,
                },
                body,
                created_at: Utc::now(),
                updated_at: None,
                can_edit: true,
                can_delete: true,
                url: None,
                resolved: None,
            };
            self.append_confirmed_comment(&request, comment);
            self.overlay = None;
            self.set_toast("Comment posted");
            return;
        }
        let Some(provider) = self.providers.get(&request.forge).cloned() else {
            self.set_toast("No provider is configured for this request");
            self.set_composer_error(
                body,
                "No provider is configured for this request".into(),
                false,
            );
            return;
        };
        self.pending_comment = Some(PendingComment {
            request: request.clone(),
            correlation_id,
            body: body.clone(),
        });
        self.overlay = Some(Overlay::Composer {
            body: body.clone(),
            error: None,
            retry_requires_refresh: false,
            button_hits: vec![],
        });
        let sender = self.events.clone();
        tokio::spawn(async move {
            let result = comment_write_with_timeout(
                provider.create_comment(&request, &body),
                Duration::from_secs(30),
            )
            .await;
            let _ = sender.send(AppEvent::CommentWrite {
                request,
                correlation_id,
                result,
            });
        });
        self.set_toast("Posting comment…");
    }
    fn set_composer_error(&mut self, body: String, error: String, retry_requires_refresh: bool) {
        match &mut self.overlay {
            Some(Overlay::Composer {
                body: current,
                error: current_error,
                retry_requires_refresh: current_retry_requires_refresh,
                ..
            }) => {
                *current = body;
                *current_error = Some(error);
                *current_retry_requires_refresh = retry_requires_refresh;
            }
            _ => {
                self.overlay = Some(Overlay::Composer {
                    body,
                    error: Some(error),
                    retry_requires_refresh,
                    button_hits: vec![],
                });
            }
        }
    }
    fn refresh_comments_after_timeout(&mut self, request: ChangeRequestId) {
        let Some(provider) = self.providers.get(&request.forge).cloned() else {
            self.set_toast("No provider is configured for this request");
            return;
        };
        let resources = self.detail_resources.entry(request.clone()).or_default();
        resources.comments = LoadState::Loading;
        let revision = resources.comments_revision;
        let sender = self.events.clone();
        tokio::spawn(async move {
            let result = provider.list_comments(&request).await;
            let _ = sender.send(AppEvent::CommentsLoaded {
                request,
                revision,
                result,
            });
        });
        self.set_toast("Refreshing comments…");
    }
    fn append_confirmed_comment(&mut self, id: &ChangeRequestId, comment: Comment) {
        let Some(request) = self.requests.iter_mut().find(|request| request.id == *id) else {
            return;
        };
        if let Some(existing) = request
            .comments
            .iter_mut()
            .find(|item| item.id == comment.id)
        {
            *existing = comment;
        } else {
            request.comments.push(comment);
        }
        request.comments.sort_by_key(|left| left.created_at);
        let comments = request.comments.clone();
        let resources = self.detail_resources.entry(id.clone()).or_default();
        resources.comments = LoadState::Loaded(comments);
        resources.comments_revision = resources.comments_revision.saturating_add(1);
        resources.refreshed_at = Some(Instant::now());
        self.comment_scroll = 0;
    }
    fn apply_review(&mut self, state: ReviewState) {
        if !self.can_review_action(state) {
            self.set_toast("This forge has not advertised this write capability");
            return;
        }
        let Some(request) = self.request_for_view().map(|item| item.id.clone()) else {
            return;
        };
        if self.demo {
            if let Some(pr) = self.requests.iter_mut().find(|pr| pr.id == request) {
                pr.review = state;
            }
            self.set_toast(match state {
                ReviewState::Approved => "Review approved",
                ReviewState::ChangesRequested => "Changes requested",
                _ => "Review submitted",
            });
            return;
        }
        let Some(provider) = self.providers.get(&request.forge).cloned() else {
            self.set_toast("No provider is configured for this request");
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
        self.set_toast("Submitting review");
    }
    fn can_review_action(&self, state: ReviewState) -> bool {
        match state {
            ReviewState::Approved => self.can(|capabilities| capabilities.approve),
            ReviewState::ChangesRequested => self.can(|capabilities| capabilities.request_changes),
            _ => self.can(|capabilities| capabilities.reviews),
        }
    }
    pub(crate) fn palette_commands(&self) -> Vec<&'static str> {
        let mut commands = Vec::new();
        if self.view == View::Dashboard && self.can_create_current_project() {
            commands.push("Create pull request");
        }
        if let Some(request) = self.request_for_view() {
            let caps = self.capabilities_for(&request.id);
            let any_edit = caps.edit_title
                || caps.edit_description
                || caps.labels
                || caps.request_reviewers
                || caps.assignees
                || caps.milestone
                || caps.draft_transition
                || (request.state == RequestState::Open && caps.close)
                || (request.state != RequestState::Open && caps.reopen);
            if self.view == View::ChangeRequestDetail(request.id.clone()) && any_edit {
                commands.push("Edit");
            }
            if self.view == View::ChangeRequestDetail(request.id.clone()) {
                if caps.edit_title {
                    commands.push("Edit title");
                }
                if caps.edit_description {
                    commands.push("Edit description");
                }
                if caps.labels {
                    commands.push("Edit labels");
                }
                if caps.request_reviewers {
                    commands.push("Edit reviewers");
                }
                if caps.assignees {
                    commands.push("Edit assignees");
                }
                if caps.milestone {
                    commands.push("Edit milestone");
                }
                if caps.draft_transition && !request.draft && request.state == RequestState::Open {
                    commands.push("Mark as draft");
                }
                if caps.draft_transition && request.draft {
                    commands.push("Mark ready for review");
                }
                if caps.close && request.state == RequestState::Open {
                    commands.push("Close request");
                }
                if caps.reopen && request.state == RequestState::Closed {
                    commands.push("Reopen request");
                }
                if MergeSession::build(request, &caps).is_some() {
                    commands.push("Merge");
                }
                if caps.auto_merge && request.state == RequestState::Open {
                    commands.push(if request.auto_merge {
                        "Disable auto-merge"
                    } else {
                        "Enable auto-merge"
                    });
                }
            }
            if caps.comments {
                commands.push("Add comment");
            }
            if caps.approve {
                commands.push("Approve");
            }
            if caps.request_changes {
                commands.push("Request changes");
            }
            if caps.request_reviewers {
                commands.push("Request reviewer");
            }
            if request.web_url.is_some() {
                commands.push("Open in browser");
            }
            if caps.ci_read {
                commands.extend(["Open pipeline", "Refresh pipeline"]);
            }
            if caps.ci_logs {
                commands.extend(["Open job logs", "Follow logs"]);
            }
            if caps.ci_retry_job {
                commands.push("Retry failed job");
            }
            if caps.ci_retry_pipeline {
                commands.push("Retry pipeline");
            }
            if caps.ci_cancel_job {
                commands.push("Cancel job");
            }
            if caps.ci_cancel_pipeline {
                commands.push("Cancel pipeline");
            }
        }
        commands.push("Refresh");
        commands
    }
    fn palette_command_count(&self, query: &str) -> usize {
        self.palette_commands()
            .iter()
            .filter(|command| command.to_lowercase().contains(&query.to_lowercase()))
            .count()
            .saturating_sub(1)
    }
    fn run_palette(&mut self, query: &str, selected: usize) {
        let command = self
            .palette_commands()
            .iter()
            .filter(|command| command.to_lowercase().contains(&query.to_lowercase()))
            .nth(selected)
            .copied();
        match command {
            Some("Create pull request") => self.open_create(),
            Some("Edit") => self.open_edit(),
            Some("Edit title") => self.open_edit_action(EditAction::Title),
            Some("Edit description") => self.open_edit_action(EditAction::Description),
            Some("Edit labels") => self.open_edit_action(EditAction::Labels),
            Some("Edit reviewers") => self.open_edit_action(EditAction::Reviewers),
            Some("Edit assignees") => self.open_edit_action(EditAction::Assignees),
            Some("Edit milestone") => self.open_edit_action(EditAction::Milestone),
            Some("Mark as draft") => self.open_edit_action(EditAction::Draft),
            Some("Mark ready for review") => self.open_edit_action(EditAction::Ready),
            Some("Close request") => self.open_edit_action(EditAction::Close),
            Some("Reopen request") => self.open_edit_action(EditAction::Reopen),
            Some("Merge") => self.open_merge(),
            Some("Enable auto-merge") => {
                if let Some(id) = self.detail_request().map(|r| r.id.clone()) {
                    self.start_auto_merge(id, true);
                }
            }
            Some("Disable auto-merge") => {
                if let Some(id) = self.detail_request().map(|r| r.id.clone()) {
                    self.start_auto_merge(id, false);
                }
            }
            Some("Add comment") => self.open_comment_composer(),
            Some("Approve") => self.apply_review(ReviewState::Approved),
            Some("Request changes") => self.apply_review(ReviewState::ChangesRequested),
            Some("Refresh") => self.request_refresh(),
            Some("Request reviewer") => self.open_edit_action(EditAction::Reviewers),
            Some(_) => {
                self.set_toast("This command is not available in the current view");
            }
            None => {}
        }
    }
    pub fn set_regions(&mut self, regions: HitRegions) {
        self.regions = regions;
    }
    pub fn handle_mouse(&mut self, event: MouseEvent) {
        if let Some(overlay) = self.overlay.take() {
            if matches!(&overlay, Overlay::Composer { .. })
                && !matches!(event.kind, MouseEventKind::Down(MouseButton::Left))
            {
                self.overlay = Some(overlay);
                return;
            }
            if !matches!(event.kind, MouseEventKind::Down(_)) {
                self.overlay = Some(overlay);
                return;
            }
            match overlay {
                Overlay::Composer {
                    body,
                    error,
                    retry_requires_refresh,
                    button_hits,
                } => {
                    if self.pending_comment.is_some() {
                        self.overlay = Some(Overlay::Composer {
                            body,
                            error,
                            retry_requires_refresh,
                            button_hits,
                        });
                        return;
                    }
                    let clicked = button_hits.iter().find_map(|(rect, action)| {
                        rect.contains(ratatui::layout::Position {
                            x: event.column,
                            y: event.row,
                        })
                        .then_some(*action)
                    });
                    match clicked {
                        Some(0) => {
                            if retry_requires_refresh
                                && let Some(request) = self.comment_retry_requires_refresh.clone()
                            {
                                self.comment_retry_draft = Some((request, body));
                            }
                        }
                        Some(1) if retry_requires_refresh => {
                            if let Some(request) = self.comment_retry_requires_refresh.clone() {
                                self.comment_retry_draft = Some((request.clone(), body));
                                self.overlay = None;
                                self.refresh_comments_after_timeout(request);
                            } else {
                                self.overlay = Some(Overlay::Composer {
                                    body,
                                    error,
                                    retry_requires_refresh,
                                    button_hits,
                                });
                            }
                        }
                        Some(1) => self.submit_comment(body),
                        _ => {
                            self.overlay = Some(Overlay::Composer {
                                body,
                                error,
                                retry_requires_refresh,
                                button_hits,
                            });
                        }
                    }
                }
                Overlay::Create(mut session) => {
                    let close = session.handle_mouse(self, event.column, event.row);
                    if self.overlay.is_none() && !close {
                        self.overlay = Some(Overlay::Create(session));
                    }
                }
                Overlay::Edit(mut session) => {
                    session.handle_mouse(self, event.column, event.row);
                    if self.overlay.is_none() {
                        self.overlay = Some(Overlay::Edit(session));
                    }
                }
                Overlay::Merge(mut session) => {
                    let close = session.handle_mouse(self, event.column, event.row);
                    if self.overlay.is_none() && !close {
                        self.overlay = Some(Overlay::Merge(session));
                    }
                }
                Overlay::Confirm(mut dialog) => {
                    if matches!(event.kind, MouseEventKind::Down(_)) {
                        for (rect, index) in dialog.button_hits.clone() {
                            if rect.contains(ratatui::layout::Position {
                                x: event.column,
                                y: event.row,
                            }) {
                                dialog.selected = index;
                                if index == 1 {
                                    self.confirm(dialog.action.clone());
                                }
                                return;
                            }
                        }
                    }
                    self.overlay = Some(Overlay::Confirm(dialog));
                }
                Overlay::BranchCleanup(mut session) => {
                    if matches!(event.kind, MouseEventKind::Down(_)) {
                        for (rect, index) in session.button_hits.clone() {
                            if rect.contains(ratatui::layout::Position {
                                x: event.column,
                                y: event.row,
                            }) {
                                session.selected = index;
                                if index > 0 && !session.pending {
                                    self.start_branch_cleanup(session);
                                } else if index == 0 {
                                    return;
                                } else {
                                    self.overlay = Some(Overlay::BranchCleanup(session));
                                }
                                return;
                            }
                        }
                    }
                    self.overlay = Some(Overlay::BranchCleanup(session));
                }
                Overlay::Palette {
                    query,
                    mut selected,
                } => {
                    if matches!(event.kind, MouseEventKind::Down(_)) {
                        if let Some((_, index)) = self.palette_hits.iter().find(|(rect, _)| {
                            rect.contains(ratatui::layout::Position {
                                x: event.column,
                                y: event.row,
                            })
                        }) {
                            selected = *index;
                            self.run_palette(&query, selected);
                        } else {
                            self.overlay = Some(Overlay::Palette { query, selected });
                        }
                    } else {
                        self.overlay = Some(Overlay::Palette { query, selected });
                    }
                }
                other => self.overlay = Some(other),
            }
            return;
        }
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
        let opened = match &self.view {
            View::ChangeRequestDetail(id) => self
                .requests
                .iter()
                .find(|request| request.id == *id)
                .cloned(),
            _ => None,
        };
        self.requests = result.requests;
        if self.demo {
            self.hydrate_demo_resources();
        } else {
            for request in &mut self.requests {
                let Some(resources) = self.detail_resources.get(&request.id) else {
                    continue;
                };
                if let LoadState::Loaded(comments) = &resources.comments {
                    request.comments = comments.clone();
                }
                if let LoadState::Loaded(reviewers) = &resources.reviews {
                    request.reviewers = reviewers.clone();
                }
                if let LoadState::Loaded(pipelines) = &resources.ci {
                    request.pipelines = pipelines.clone();
                    request.ci = summarize_ci(pipelines);
                }
            }
        }
        if let Some(opened) = opened
            && !self.requests.iter().any(|request| request.id == opened.id)
        {
            self.requests.push(opened);
        }
        self.health = result.health;
        self.stale = result.from_cache;
        self.last_refresh = Some(Instant::now());
        if self.selected >= self.visible().len() {
            self.selected = self.visible().len().saturating_sub(1);
        }
    }
    pub fn apply_comment_write(
        &mut self,
        request: ChangeRequestId,
        correlation_id: OpId,
        result: Result<Comment, forge::ForgeError>,
    ) {
        let Some(pending) = self.pending_comment.as_ref() else {
            return;
        };
        if pending.request != request || pending.correlation_id != correlation_id {
            return;
        }
        let pending = self.pending_comment.take().unwrap();
        match result {
            Ok(comment) => {
                if self.comment_retry_requires_refresh.as_ref() == Some(&request) {
                    self.comment_retry_requires_refresh = None;
                }
                if self
                    .comment_retry_draft
                    .as_ref()
                    .is_some_and(|(draft_request, _)| draft_request == &request)
                {
                    self.comment_retry_draft = None;
                }
                self.append_confirmed_comment(&request, comment);
                if matches!(self.overlay, Some(Overlay::Composer { .. })) {
                    self.overlay = None;
                }
                self.set_toast("Comment posted");
            }
            Err(error) => {
                let retry_requires_refresh = matches!(error, forge::ForgeError::CommentTimedOut);
                if retry_requires_refresh {
                    let resources = self.detail_resources.entry(request.clone()).or_default();
                    resources.comments_revision = resources.comments_revision.saturating_add(1);
                    resources.comments = LoadState::NotLoaded;
                    resources.refreshed_at = None;
                    self.comment_retry_requires_refresh = Some(request.clone());
                    self.comment_retry_draft = Some((request.clone(), pending.body.clone()));
                }
                let detail = error_summary(&error);
                let message = format!("Comment failed: {detail}");
                self.set_toast(message.clone());
                self.set_composer_error(pending.body, detail, retry_requires_refresh);
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
                self.set_toast("Review submitted");
            }
            Err(error) => self.set_toast(format!("Review failed: {error}")),
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
                    item.ci = summarize_ci(&pipelines);
                    item.pipelines = pipelines.clone();
                }
                let resources = self.detail_resources.entry(request).or_default();
                resources.ci = LoadState::Loaded(pipelines);
                resources.refreshed_at = Some(Instant::now());
            }
            Err(error) => {
                let state = load_failure(error);
                let resources = self.detail_resources.entry(request).or_default();
                resources.ci = state;
                resources.refreshed_at = Some(Instant::now());
            }
        }
    }
    pub fn apply_detail_request(
        &mut self,
        id: ChangeRequestId,
        result: Result<ChangeRequest, forge::ForgeError>,
    ) {
        match result {
            Ok(mut request) => {
                if let Some(resources) = self.detail_resources.get(&id) {
                    if let LoadState::Loaded(comments) = &resources.comments {
                        request.comments = comments.clone();
                    }
                    if let LoadState::Loaded(reviewers) = &resources.reviews {
                        request.reviewers = reviewers.clone();
                    }
                    if let LoadState::Loaded(pipelines) = &resources.ci {
                        request.pipelines = pipelines.clone();
                        request.ci = summarize_ci(pipelines);
                    }
                }
                self.reconcile_request(request);
                let resources = self.detail_resources.entry(id.clone()).or_default();
                resources.request = LoadState::Loaded(());
                resources.refreshed_at = Some(Instant::now());
            }
            Err(error) => {
                let resources = self.detail_resources.entry(id).or_default();
                resources.request = load_failure(error);
                resources.refreshed_at = Some(Instant::now());
            }
        }
    }
    pub fn apply_comments(
        &mut self,
        id: ChangeRequestId,
        revision: u64,
        result: Result<Vec<Comment>, forge::ForgeError>,
    ) {
        if self
            .detail_resources
            .get(&id)
            .is_some_and(|resources| revision < resources.comments_revision)
        {
            return;
        }
        match result {
            Ok(comments) => {
                let comments = deduplicate_comments(comments);
                if let Some(request) = self.requests.iter_mut().find(|request| request.id == id) {
                    request.comments = comments.clone();
                }
                let resources = self.detail_resources.entry(id.clone()).or_default();
                resources.comments = LoadState::Loaded(comments);
                resources.refreshed_at = Some(Instant::now());
                if self.comment_retry_requires_refresh.as_ref() == Some(&id) {
                    self.comment_retry_requires_refresh = None;
                    if self
                        .request_for_view()
                        .is_some_and(|request| request.id == id)
                        && let Some(Overlay::Composer {
                            error,
                            retry_requires_refresh,
                            ..
                        }) = &mut self.overlay
                        && *retry_requires_refresh
                    {
                        *retry_requires_refresh = false;
                        *error = None;
                    }
                }
            }
            Err(error) => {
                let resources = self.detail_resources.entry(id).or_default();
                resources.comments = load_failure(error);
                resources.refreshed_at = Some(Instant::now());
            }
        }
    }
    pub fn apply_reviews(
        &mut self,
        id: ChangeRequestId,
        result: Result<Vec<Reviewer>, forge::ForgeError>,
    ) {
        match result {
            Ok(reviewers) => {
                if let Some(request) = self.requests.iter_mut().find(|request| request.id == id) {
                    request.reviewers = reviewers.clone();
                }
                let resources = self.detail_resources.entry(id).or_default();
                resources.reviews = LoadState::Loaded(reviewers);
                resources.refreshed_at = Some(Instant::now());
            }
            Err(error) => {
                let resources = self.detail_resources.entry(id).or_default();
                resources.reviews = load_failure(error);
                resources.refreshed_at = Some(Instant::now());
            }
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
            Err(error) => self.set_toast(format!("Pipeline refresh failed: {error}")),
        }
    }
    pub fn apply_ci_action(&mut self, _action: CiAction, result: Result<(), forge::ForgeError>) {
        match result {
            Ok(()) => {
                self.set_toast("CI action completed. Refreshing pipeline.");
                self.load_pipeline();
            }
            Err(error) => self.set_toast(format!("CI action failed: {error}")),
        }
    }
}

fn begin_load<T>(state: &mut LoadState<T>, force: bool, stale: bool) -> bool {
    if matches!(state, LoadState::Loading) {
        return false;
    }
    if force || stale || matches!(state, LoadState::NotLoaded | LoadState::Failed(_)) {
        *state = LoadState::Loading;
        true
    } else {
        false
    }
}

fn load_failure<T>(error: forge::ForgeError) -> LoadState<T> {
    match error {
        forge::ForgeError::Unsupported => LoadState::Unsupported,
        error => LoadState::Failed(error_summary(&error)),
    }
}

fn deduplicate_comments(comments: Vec<Comment>) -> Vec<Comment> {
    let mut result: Vec<Comment> = Vec::with_capacity(comments.len());
    for comment in comments {
        if let Some(index) = result.iter().position(|current| current.id == comment.id) {
            result[index] = comment;
        } else {
            result.push(comment);
        }
    }
    result.sort_by_key(|left| left.created_at);
    result
}

pub(crate) fn error_summary(error: &forge::ForgeError) -> String {
    match error {
        forge::ForgeError::AuthenticationRequired(_) => "authentication required".into(),
        forge::ForgeError::Unavailable(_) => "network or provider unavailable".into(),
        forge::ForgeError::CommentTimedOut => {
            "submission timed out; the server may have accepted it. Refresh comments before retrying"
                .into()
        }
        forge::ForgeError::PermissionDenied => "permission denied".into(),
        forge::ForgeError::RateLimited { .. } => "rate limited".into(),
        forge::ForgeError::NotFound => "request or repository not found".into(),
        forge::ForgeError::Validation(message) => format!("validation failed: {message}"),
        forge::ForgeError::Conflict => "conflict; refresh and try again".into(),
        forge::ForgeError::Unsupported => "unsupported by this provider".into(),
        forge::ForgeError::JobNotRetryable => "job cannot be retried".into(),
        forge::ForgeError::PipelineNotCancelable => "pipeline cannot be cancelled".into(),
        forge::ForgeError::LogsUnavailable => "logs are unavailable".into(),
    }
}

async fn comment_write_with_timeout<F>(
    write: F,
    limit: Duration,
) -> Result<Comment, forge::ForgeError>
where
    F: Future<Output = Result<Comment, forge::ForgeError>>,
{
    match tokio::time::timeout(limit, write).await {
        Ok(result) => result,
        Err(_) => Err(forge::ForgeError::CommentTimedOut),
    }
}

fn summarize_ci(pipelines: &[Pipeline]) -> CiState {
    let states = pipelines
        .iter()
        .map(|pipeline| pipeline.status.ci_state())
        .collect::<Vec<_>>();
    if states.contains(&CiState::Failed) {
        CiState::Failed
    } else if states.contains(&CiState::Running) {
        CiState::Running
    } else if states.contains(&CiState::Pending) {
        CiState::Pending
    } else if states.contains(&CiState::Passed) {
        CiState::Passed
    } else {
        CiState::None
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
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    struct DetailSpyProvider {
        name: String,
        calls: Arc<std::sync::Mutex<Vec<&'static str>>>,
    }

    #[async_trait::async_trait]
    impl ForgeProvider for DetailSpyProvider {
        fn name(&self) -> &str {
            &self.name
        }

        fn capabilities(&self) -> forge::ForgeCapabilities {
            forge::ForgeCapabilities {
                ci_read: true,
                ..forge::ForgeCapabilities::default()
            }
        }

        async fn list_change_requests(&self) -> Result<Vec<ChangeRequest>, forge::ForgeError> {
            self.calls.lock().unwrap().push("list_change_requests");
            Ok(vec![])
        }

        async fn get_change_request(
            &self,
            id: &ChangeRequestId,
        ) -> Result<ChangeRequest, forge::ForgeError> {
            self.calls.lock().unwrap().push("get_change_request");
            forge::demo::change_requests()
                .into_iter()
                .find(|request| request.id == *id)
                .ok_or(forge::ForgeError::NotFound)
        }

        async fn list_reviews(
            &self,
            _id: &ChangeRequestId,
        ) -> Result<Vec<Reviewer>, forge::ForgeError> {
            self.calls.lock().unwrap().push("list_reviews");
            Ok(vec![])
        }

        async fn list_comments(
            &self,
            _id: &ChangeRequestId,
        ) -> Result<Vec<Comment>, forge::ForgeError> {
            self.calls.lock().unwrap().push("list_comments");
            Ok(vec![])
        }

        async fn list_pipelines(
            &self,
            _id: &ChangeRequestId,
        ) -> Result<Vec<Pipeline>, forge::ForgeError> {
            self.calls.lock().unwrap().push("list_pipelines");
            Ok(vec![])
        }
    }

    struct CommentSpyProvider {
        calls: Arc<AtomicUsize>,
        fail: bool,
    }

    #[async_trait::async_trait]
    impl ForgeProvider for CommentSpyProvider {
        fn name(&self) -> &str {
            "github"
        }

        fn capabilities(&self) -> forge::ForgeCapabilities {
            forge::ForgeCapabilities {
                comments: true,
                ..forge::ForgeCapabilities::default()
            }
        }

        async fn list_change_requests(&self) -> Result<Vec<ChangeRequest>, forge::ForgeError> {
            Ok(vec![])
        }

        async fn list_comments(
            &self,
            _id: &ChangeRequestId,
        ) -> Result<Vec<Comment>, forge::ForgeError> {
            Ok(vec![])
        }

        async fn create_comment(
            &self,
            _id: &ChangeRequestId,
            body: &str,
        ) -> Result<Comment, forge::ForgeError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            if self.fail {
                Err(forge::ForgeError::PermissionDenied)
            } else {
                Ok(Comment {
                    id: "server-comment-1".into(),
                    author: Person::named("jack"),
                    body: body.into(),
                    created_at: Utc::now(),
                    updated_at: None,
                    can_edit: false,
                    can_delete: false,
                    url: None,
                    resolved: None,
                })
            }
        }
    }

    fn live_comment_app(
        calls: Arc<AtomicUsize>,
        fail: bool,
    ) -> (App, ChangeRequestId, mpsc::UnboundedReceiver<AppEvent>) {
        let mut app = App::test_app();
        let (events, receiver) = mpsc::unbounded_channel();
        app.events = events;
        app.demo = false;
        let id = app.requests[0].id.clone();
        app.view = View::ChangeRequestDetail(id.clone());
        app.providers.insert(
            id.forge.clone(),
            Arc::new(CommentSpyProvider { calls, fail }),
        );
        (app, id, receiver)
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
    async fn composer_input_is_multiline_unicode_and_does_not_include_the_opening_key() {
        let mut app = App::test_app();
        app.handle_key(KeyCode::Char('c'));
        for character in "café 🚀".chars() {
            app.handle_key(KeyCode::Char(character));
        }
        app.handle_key(KeyCode::Enter);
        for character in "第二行".chars() {
            app.handle_key(KeyCode::Char(character));
        }

        let Some(Overlay::Composer { body, .. }) = &app.overlay else {
            panic!("composer should remain open while typing")
        };
        assert_eq!(body, "café 🚀\n第二行");
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL));

        let posted = app.selected_request().unwrap().comments.last().unwrap();
        assert_eq!(posted.body, "café 🚀\n第二行");
    }

    #[tokio::test]
    async fn key_repeat_events_do_not_type_or_submit_a_comment() {
        let mut app = App::test_app();
        let before = app.selected_request().unwrap().comments.len();
        app.handle_key(KeyCode::Char('c'));
        app.handle_key_event(KeyEvent::new_with_kind(
            KeyCode::Char('x'),
            KeyModifiers::NONE,
            KeyEventKind::Repeat,
        ));
        app.handle_key_event(KeyEvent::new_with_kind(
            KeyCode::Enter,
            KeyModifiers::CONTROL,
            KeyEventKind::Repeat,
        ));

        assert!(matches!(app.overlay, Some(Overlay::Composer { ref body, .. }) if body.is_empty()));
        assert_eq!(app.selected_request().unwrap().comments.len(), before);
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
        app.view = View::ChangeRequestDetail(id);
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
    async fn project_n_and_palette_open_the_same_create_flow_when_empty() {
        let (sender, _) = mpsc::unbounded_channel();
        let mut app = App::new(Config::default(), true, None, sender)
            .await
            .unwrap();
        app.requests.clear();

        app.handle_key(KeyCode::Char('n'));
        let (first_repository, first_kind, first_stage, first_push_state) =
            match app.overlay.as_ref() {
                Some(Overlay::Create(session)) => (
                    session.repository.clone(),
                    session.kind,
                    session.stage.clone(),
                    session.push_state.clone(),
                ),
                other => panic!("expected create workflow from n, got {other:?}"),
            };
        assert_eq!(first_repository, "jack/quickdrop");
        assert_eq!(first_stage, create::CreateStage::Preflight);
        assert_eq!(first_push_state, create::PushState::WaitingForPush);

        app.overlay = None;
        app.handle_key(KeyCode::Char(':'));
        for key in "create pull request".chars() {
            app.handle_key(KeyCode::Char(key));
        }
        app.handle_key(KeyCode::Enter);

        let second = match app.overlay.as_ref() {
            Some(Overlay::Create(session)) => session,
            other => panic!("expected create workflow from palette, got {other:?}"),
        };
        assert_eq!(second.repository, first_repository);
        assert_eq!(second.kind, first_kind);
        assert_eq!(second.stage, first_stage);
        assert_eq!(second.push_state, first_push_state);
    }

    #[tokio::test]
    async fn demo_create_push_and_success_reach_the_new_detail_through_app_events() {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let mut app = App::new(Config::default(), true, None, sender)
            .await
            .unwrap();
        app.requests.clear();
        app.handle_key(KeyCode::Char('n'));
        app.handle_key(KeyCode::Right);
        app.handle_key(KeyCode::Enter);

        let AppEvent::PushCompleted { op, branch, result } = receiver.recv().await.unwrap() else {
            panic!("expected async push completion");
        };
        app.apply_push(op, branch, result);
        assert!(matches!(
            app.overlay,
            Some(Overlay::Create(ref session)) if session.stage == create::CreateStage::Fields
        ));

        app.handle_key(KeyCode::Down);
        app.handle_key(KeyCode::Enter);
        for _ in 0.."New reader".chars().count() {
            app.handle_key(KeyCode::Backspace);
        }
        for character in "New demo request".chars() {
            app.handle_key(KeyCode::Char(character));
        }
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL));
        app.handle_key(KeyCode::Right);
        app.handle_key(KeyCode::Enter);

        let AppEvent::CreateCompleted { op, result } = receiver.recv().await.unwrap() else {
            panic!("expected async create completion");
        };
        app.apply_create(op, result);
        let View::ChangeRequestDetail(id) = app.view.clone() else {
            panic!("successful creation must open the new detail view");
        };
        let created = app
            .requests
            .iter()
            .find(|request| request.id == id)
            .expect("created request is reconciled immediately");
        assert_eq!(created.title, "New demo request");
        assert_eq!(id.number, 1);
        assert!(app.overlay.is_none());
        assert_eq!(app.toast.as_deref(), Some("Created #1"));
    }

    #[tokio::test]
    async fn demo_lifecycle_writes_reconcile_draft_ready_close_and_reopen() {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let mut app = App::new(Config::default(), true, None, sender)
            .await
            .unwrap();
        let id = app.requests[0].id.clone();
        app.view = View::ChangeRequestDetail(id.clone());
        for (action, patch, expected_state, expected_draft) in [
            (
                LifecycleAction::Draft,
                forge::RequestPatch {
                    draft: Some(true),
                    ..forge::RequestPatch::default()
                },
                RequestState::Open,
                true,
            ),
            (
                LifecycleAction::Ready,
                forge::RequestPatch {
                    draft: Some(false),
                    ..forge::RequestPatch::default()
                },
                RequestState::Open,
                false,
            ),
            (
                LifecycleAction::Close,
                forge::RequestPatch {
                    state: Some(RequestState::Closed),
                    ..forge::RequestPatch::default()
                },
                RequestState::Closed,
                false,
            ),
            (
                LifecycleAction::Reopen,
                forge::RequestPatch {
                    state: Some(RequestState::Open),
                    ..forge::RequestPatch::default()
                },
                RequestState::Open,
                false,
            ),
        ] {
            app.start_update(id.clone(), patch, action);
            let AppEvent::RequestWriteCompleted {
                id: event_id,
                op,
                action: event_action,
                result,
            } = receiver.recv().await.unwrap()
            else {
                panic!("expected lifecycle write completion");
            };
            assert_eq!(event_action, action);
            app.apply_request_write(event_id, op, event_action, result);
            let request = app
                .requests
                .iter()
                .find(|request| request.id == id)
                .unwrap();
            assert_eq!(request.state, expected_state);
            assert_eq!(request.draft, expected_draft);
        }
    }

    #[tokio::test]
    async fn demo_merge_success_updates_detail_and_adds_merge_activity() {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let mut app = App::new(Config::default(), true, None, sender)
            .await
            .unwrap();
        let id = app.requests[0].id.clone();
        app.view = View::ChangeRequestDetail(id.clone());
        app.handle_key(KeyCode::Char('M'));
        app.handle_key(KeyCode::Right);
        app.handle_key(KeyCode::Enter);
        assert!(
            matches!(app.overlay, Some(Overlay::Merge(ref session)) if session.stage == crate::merge::MergeStage::Confirm),
            "unexpected merge overlay state: {:?}",
            app.overlay
        );
        app.handle_key(KeyCode::Right);
        app.handle_key(KeyCode::Enter);

        let AppEvent::MergeCompleted {
            id: event_id,
            op,
            result,
        } = receiver.recv().await.unwrap()
        else {
            panic!("expected async merge completion");
        };
        app.apply_merge(event_id, op, result);
        let request = app
            .requests
            .iter()
            .find(|request| request.id == id)
            .unwrap();
        assert_eq!(request.state, RequestState::Merged);
        assert!(request.merged_sha.is_some());
        assert_eq!(app.activity.get(&id).unwrap().len(), 1);
        assert!(
            matches!(app.overlay, Some(Overlay::BranchCleanup(ref session)) if session.selected == 0)
        );
    }

    #[tokio::test]
    async fn merge_overlay_consumes_keys_and_mouse_scroll_without_touching_detail() {
        let (sender, _) = mpsc::unbounded_channel();
        let mut app = App::new(Config::default(), true, None, sender)
            .await
            .unwrap();
        let id = app.requests[0].id.clone();
        app.view = View::ChangeRequestDetail(id.clone());
        let mut session = MergeSession::build(
            app.requests
                .iter()
                .find(|request| request.id == id)
                .unwrap(),
            &app.capabilities_for(&id),
        )
        .unwrap();
        session.button_hits.push((Rect::new(10, 10, 10, 1), 1));
        app.overlay = Some(Overlay::Merge(session));

        app.handle_key(KeyCode::Char('e'));
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 11,
            row: 10,
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(app.view, View::ChangeRequestDetail(id));
        assert!(
            matches!(app.overlay, Some(Overlay::Merge(ref session)) if session.stage == crate::merge::MergeStage::Preflight && !session.write.is_pending())
        );
    }

    #[tokio::test]
    async fn create_overlay_consumes_detail_shortcuts_and_escape_closes_only_the_overlay() {
        let (sender, _) = mpsc::unbounded_channel();
        let mut app = App::new(Config::default(), true, None, sender)
            .await
            .unwrap();
        let id = app.requests[0].id.clone();
        app.view = View::ChangeRequestDetail(id.clone());
        app.open_create();

        app.handle_key(KeyCode::Char('M'));
        assert!(matches!(app.overlay, Some(Overlay::Create(_))));
        app.handle_key(KeyCode::Esc);

        assert!(app.overlay.is_none());
        assert_eq!(app.view, View::ChangeRequestDetail(id));
    }

    #[tokio::test]
    async fn merge_palette_waits_for_repository_policy_and_hides_disabled_actions() {
        let (sender, _) = mpsc::unbounded_channel();
        let mut app = App::new(Config::default(), true, None, sender)
            .await
            .unwrap();
        app.demo = false;
        let id = app.requests[0].id.clone();
        app.view = View::ChangeRequestDetail(id.clone());
        app.providers.insert(
            id.forge.clone(),
            Arc::new(edit::RecordingProvider::default()),
        );

        assert!(!app.palette_commands().contains(&"Merge"));
        assert!(!app.palette_commands().contains(&"Enable auto-merge"));
        app.apply_repository_info(
            id.forge.clone(),
            id.repository.clone(),
            Ok(forge::RepositoryInfo {
                allow_merge_commit: Some(false),
                allow_squash_merge: Some(true),
                allow_rebase_merge: Some(false),
                allow_auto_merge: Some(false),
                ..forge::RepositoryInfo::default()
            }),
        );

        assert!(app.palette_commands().contains(&"Merge"));
        assert!(!app.palette_commands().contains(&"Enable auto-merge"));
        app.open_merge();
        let Some(Overlay::Merge(session)) = app.overlay.clone() else {
            panic!("repository policy should leave the supported squash strategy available");
        };
        assert_eq!(session.strategies, vec![MergeStrategy::Squash]);
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

    #[tokio::test]
    async fn opening_live_detail_fetches_request_reviews_and_pipelines() {
        let mut app = App::test_app();
        app.demo = false;
        let id = app.requests[0].id.clone();
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        app.providers.insert(
            id.forge.clone(),
            Arc::new(DetailSpyProvider {
                name: id.forge.clone(),
                calls: calls.clone(),
            }),
        );

        app.open_selected();
        let resources = app.detail_resources_for(&id).unwrap();
        assert!(matches!(&resources.request, LoadState::Loading));
        assert!(matches!(&resources.comments, LoadState::Loading));
        assert!(matches!(&resources.reviews, LoadState::Loading));
        assert!(matches!(&resources.ci, LoadState::Loading));
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;

        let calls = calls.lock().unwrap().clone();
        assert!(
            calls.contains(&"get_change_request"),
            "calls were {calls:?}"
        );
        assert!(calls.contains(&"list_comments"), "calls were {calls:?}");
        assert!(calls.contains(&"list_reviews"), "calls were {calls:?}");
        assert!(calls.contains(&"list_pipelines"), "calls were {calls:?}");
    }

    #[tokio::test]
    async fn demo_detail_resources_start_loaded_from_the_fixtures() {
        let (events, _receiver) = mpsc::unbounded_channel();
        let app = App::new(Config::default(), true, None, events)
            .await
            .unwrap();
        for request in &app.requests {
            let resources = app.detail_resources_for(&request.id).unwrap();
            assert!(matches!(&resources.request, LoadState::Loaded(())));
            assert!(matches!(&resources.comments, LoadState::Loaded(_)));
            assert!(matches!(&resources.reviews, LoadState::Loaded(_)));
            assert!(matches!(&resources.ci, LoadState::Loaded(_)));
        }
    }

    #[tokio::test]
    async fn detail_refresh_is_targeted_and_load_states_keep_empty_failed_and_unsupported_distinct()
    {
        let mut app = App::test_app();
        app.demo = false;
        let id = app.requests[0].id.clone();
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let other_calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        app.providers.insert(
            id.forge.clone(),
            Arc::new(DetailSpyProvider {
                name: id.forge.clone(),
                calls: calls.clone(),
            }),
        );
        app.providers.insert(
            "other".into(),
            Arc::new(DetailSpyProvider {
                name: "other".into(),
                calls: other_calls.clone(),
            }),
        );

        app.open_selected();
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        // Model the first asynchronous load completing before the user presses r.
        // This test helper drops its event receiver, so apply the loaded baseline
        // directly and isolate the calls caused by the targeted refresh.
        let resources = app.detail_resources.get_mut(&id).unwrap();
        resources.request = LoadState::Loaded(());
        resources.comments = LoadState::Loaded(vec![]);
        resources.reviews = LoadState::Loaded(vec![]);
        resources.ci = LoadState::Loaded(vec![]);
        calls.lock().unwrap().clear();
        app.handle_key(KeyCode::Char('r'));
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;

        let calls = calls.lock().unwrap().clone();
        assert!(
            calls.contains(&"get_change_request"),
            "calls were {calls:?}"
        );
        assert!(calls.contains(&"list_comments"), "calls were {calls:?}");
        assert!(calls.contains(&"list_reviews"), "calls were {calls:?}");
        assert!(calls.contains(&"list_pipelines"), "calls were {calls:?}");
        assert!(
            !calls.contains(&"list_change_requests"),
            "calls were {calls:?}"
        );
        assert!(other_calls.lock().unwrap().is_empty());

        app.apply_comments(id.clone(), 0, Ok(vec![]));
        app.apply_reviews(id.clone(), Err(forge::ForgeError::Unsupported));
        app.apply_pipelines(
            id.clone(),
            Err(forge::ForgeError::AuthenticationRequired("github".into())),
        );
        let resources = app.detail_resources_for(&id).unwrap();
        assert!(matches!(&resources.comments, LoadState::Loaded(comments) if comments.is_empty()));
        assert!(matches!(&resources.reviews, LoadState::Unsupported));
        assert!(
            matches!(&resources.ci, LoadState::Failed(error) if error == "authentication required")
        );
    }

    #[test]
    fn live_refresh_restores_loaded_detail_resources_onto_list_requests() {
        let mut app = App::test_app();
        app.demo = false;
        let mut request = app.requests[0].clone();
        let id = request.id.clone();
        request.comments.clear();
        request.reviewers.clear();
        request.pipelines.clear();
        request.ci = CiState::None;

        let created_at = Utc::now();
        let comment = Comment {
            id: "cached-comment".into(),
            author: Person::named("alice"),
            body: "cached detail comment".into(),
            created_at,
            updated_at: None,
            can_edit: false,
            can_delete: false,
            url: None,
            resolved: None,
        };
        let reviewer = Reviewer {
            person: Person::named("bob"),
            state: ReviewState::Approved,
        };
        let pipeline = Pipeline {
            id: PipelineId {
                forge: id.forge.clone(),
                repository: id.repository.clone(),
                value: "cached-pipeline".into(),
            },
            name: "cached pipeline".into(),
            ref_name: "main".into(),
            sha: "abc123".into(),
            status: PipelineStatus::Success,
            created_at,
            started_at: None,
            finished_at: None,
            stages: vec![],
            jobs: vec![],
            url: None,
            environment: None,
        };
        let mut resources = DetailResources::default();
        resources.comments = LoadState::Loaded(vec![comment.clone()]);
        resources.reviews = LoadState::Loaded(vec![reviewer.clone()]);
        resources.ci = LoadState::Loaded(vec![pipeline.clone()]);
        app.detail_resources.insert(id.clone(), resources);

        app.apply_refresh(RefreshResult {
            requests: vec![request],
            health: vec![],
            from_cache: false,
        });

        let refreshed = app.requests.iter().find(|item| item.id == id).unwrap();
        assert_eq!(refreshed.comments.len(), 1);
        assert_eq!(refreshed.comments[0].id, comment.id);
        assert_eq!(refreshed.comments[0].body, comment.body);
        assert_eq!(refreshed.reviewers.len(), 1);
        assert_eq!(refreshed.reviewers[0].person.login, "bob");
        assert_eq!(refreshed.pipelines.len(), 1);
        assert_eq!(refreshed.pipelines[0].id.value, pipeline.id.value);
        assert_eq!(refreshed.ci, CiState::Passed);
    }

    #[tokio::test]
    async fn reopening_loaded_detail_does_not_extend_freshness_before_results_arrive() {
        let mut app = App::test_app();
        app.demo = false;
        let id = app.requests[0].id.clone();
        let refreshed_at = Instant::now() - Duration::from_secs(120);
        let resources = app.detail_resources.entry(id.clone()).or_default();
        resources.request = LoadState::Loaded(());
        resources.comments = LoadState::Loaded(vec![]);
        resources.reviews = LoadState::Loaded(vec![]);
        resources.ci = LoadState::Loaded(vec![]);
        resources.refreshed_at = Some(refreshed_at);

        app.hydrate_detail(id.clone(), false);

        assert_eq!(
            app.detail_resources.get(&id).unwrap().refreshed_at,
            Some(refreshed_at)
        );
    }

    #[test]
    fn toast_state_rejects_empty_and_whitespace_messages() {
        let mut app = App::test_app();
        app.set_toast("");
        assert!(app.toast().is_none());
        app.set_toast(" \t\n ");
        assert!(app.toast().is_none());

        app.set_toast("Comment posted");
        assert_eq!(app.toast(), Some("Comment posted"));
        app.set_toast("Comment failed: permission denied");
        assert_eq!(app.toast(), Some("Comment failed: permission denied"));
    }

    #[tokio::test]
    async fn comment_submission_pending_guard_prevents_duplicate_provider_calls() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (mut app, _, mut receiver) = live_comment_app(calls.clone(), false);

        app.submit_comment("one comment".into());
        app.submit_comment("one comment".into());
        while tokio::time::timeout(std::time::Duration::from_millis(40), receiver.recv())
            .await
            .is_ok_and(|event| event.is_some())
        {}
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;

        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn stalled_comment_write_returns_refresh_before_retry_guidance() {
        let result = comment_write_with_timeout(
            std::future::pending::<Result<Comment, forge::ForgeError>>(),
            Duration::from_millis(1),
        )
        .await;
        assert!(matches!(result, Err(forge::ForgeError::CommentTimedOut)));
        assert!(
            error_summary(&forge::ForgeError::CommentTimedOut)
                .contains("Refresh comments before retrying")
        );
    }

    #[tokio::test]
    async fn stale_comment_load_cannot_remove_a_confirmed_comment() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (mut app, id, _) = live_comment_app(calls, false);
        let correlation_id = OpId::next();
        app.pending_comment = Some(PendingComment {
            request: id.clone(),
            correlation_id,
            body: "confirmed after the fetch began".into(),
        });
        app.detail_resources.entry(id.clone()).or_default().comments = LoadState::Loading;
        let comment = Comment {
            id: "confirmed-comment".into(),
            author: Person::named("jack"),
            body: "confirmed after the fetch began".into(),
            created_at: Utc::now(),
            updated_at: None,
            can_edit: false,
            can_delete: false,
            url: None,
            resolved: None,
        };

        app.apply_comment_write(id.clone(), correlation_id, Ok(comment.clone()));
        app.apply_comments(id.clone(), 0, Ok(vec![]));

        assert!(
            app.requests
                .iter()
                .find(|request| request.id == id)
                .unwrap()
                .comments
                .iter()
                .any(|loaded| loaded.id == comment.id)
        );
        assert!(matches!(
            &app.detail_resources[&id].comments,
            LoadState::Loaded(comments) if comments.iter().any(|loaded| loaded.id == comment.id)
        ));
    }

    #[tokio::test]
    async fn timed_out_comment_requires_refresh_before_retry_and_keeps_draft() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (mut app, id, mut receiver) = live_comment_app(calls.clone(), false);
        let correlation_id = OpId::next();
        app.pending_comment = Some(PendingComment {
            request: id.clone(),
            correlation_id,
            body: "ambiguous timeout draft".into(),
        });
        app.overlay = Some(Overlay::Composer {
            body: "ambiguous timeout draft".into(),
            error: None,
            retry_requires_refresh: false,
            button_hits: vec![(Rect::new(2, 2, 24, 1), 1)],
        });

        app.apply_comment_write(
            id.clone(),
            correlation_id,
            Err(forge::ForgeError::CommentTimedOut),
        );
        assert!(app.toast().unwrap().contains("server may have accepted it"));
        assert!(matches!(
            &app.overlay,
            Some(Overlay::Composer { retry_requires_refresh: true, error: Some(error), .. })
                if error.contains("Refresh comments before retrying")
        ));

        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(receiver.try_recv().is_err());
        app.handle_key(KeyCode::Esc);
        assert!(app.overlay.is_none());
        assert_eq!(
            app.comment_retry_draft,
            Some((id.clone(), "ambiguous timeout draft".into()))
        );
        app.handle_key(KeyCode::Char('c'));
        assert!(matches!(
            &app.overlay,
            Some(Overlay::Composer { body, retry_requires_refresh: true, .. })
                if body == "ambiguous timeout draft"
        ));
        if let Some(Overlay::Composer { button_hits, .. }) = &mut app.overlay {
            *button_hits = vec![(Rect::new(2, 2, 24, 1), 1)];
        }
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 3,
            row: 2,
            modifiers: KeyModifiers::NONE,
        });
        assert!(app.overlay.is_none());
        let Some(AppEvent::CommentsLoaded {
            request,
            revision,
            result,
        }) = receiver.recv().await
        else {
            panic!("refresh comments should return a result")
        };
        assert_eq!(revision, 1);
        app.apply_comments(request, revision, result);

        assert!(app.comment_retry_requires_refresh.is_none());
        app.handle_key(KeyCode::Char('c'));
        assert!(matches!(
            &app.overlay,
            Some(Overlay::Composer { body, retry_requires_refresh: false, .. })
                if body == "ambiguous timeout draft"
        ));
    }

    #[tokio::test]
    async fn comment_retry_state_for_one_request_survives_another_request_success() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (mut app, request_a, mut receiver) = live_comment_app(calls, false);
        let mut request_b_model = app.requests[0].clone();
        request_b_model.id.number += 1;
        let request_b = request_b_model.id.clone();
        app.requests.push(request_b_model);

        let timeout_id = OpId::next();
        app.pending_comment = Some(PendingComment {
            request: request_a.clone(),
            correlation_id: timeout_id,
            body: "request A draft".into(),
        });
        app.overlay = Some(Overlay::Composer {
            body: "request A draft".into(),
            error: None,
            retry_requires_refresh: false,
            button_hits: vec![],
        });
        app.apply_comment_write(
            request_a.clone(),
            timeout_id,
            Err(forge::ForgeError::CommentTimedOut),
        );

        app.view = View::ChangeRequestDetail(request_b.clone());
        app.submit_comment("request B comment".into());
        let Some(AppEvent::CommentWrite {
            request,
            correlation_id,
            result,
        }) = receiver.recv().await
        else {
            panic!("request B should complete independently")
        };
        app.apply_comment_write(request, correlation_id, result);

        assert_eq!(app.comment_retry_requires_refresh, Some(request_a.clone()));
        assert_eq!(
            app.comment_retry_draft,
            Some((request_a, "request A draft".into()))
        );
    }

    #[tokio::test]
    async fn refreshed_comments_clear_the_guard_in_an_open_composer() {
        let mut app = App::test_app();
        let id = app.requests[0].id.clone();
        app.view = View::ChangeRequestDetail(id.clone());
        app.comment_retry_requires_refresh = Some(id.clone());
        app.comment_retry_draft = Some((id.clone(), "keep this draft".into()));
        app.open_comment_composer();

        app.apply_comments(id, 0, Ok(vec![]));

        assert!(matches!(
            &app.overlay,
            Some(Overlay::Composer {
                body,
                error: None,
                retry_requires_refresh: false,
                ..
            }) if body == "keep this draft"
        ));
    }

    #[tokio::test]
    async fn successful_comment_reconciles_provider_id_once_and_closes_the_composer() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (mut app, id, mut receiver) = live_comment_app(calls, false);
        let before = app
            .requests
            .iter()
            .find(|request| request.id == id)
            .unwrap()
            .comments
            .len();
        app.overlay = Some(Overlay::Composer {
            body: "posted once".into(),
            error: None,
            retry_requires_refresh: false,
            button_hits: vec![],
        });

        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL));
        let Some(AppEvent::CommentWrite {
            request,
            correlation_id,
            result,
        }) = receiver.recv().await
        else {
            panic!("expected the normalized provider comment")
        };
        let stale_id = correlation_id;
        app.apply_comment_write(request.clone(), correlation_id, result);
        app.apply_comment_write(request, stale_id, Err(forge::ForgeError::Unsupported));

        let comments = &app
            .requests
            .iter()
            .find(|item| item.id == id)
            .unwrap()
            .comments;
        assert_eq!(comments.len(), before + 1);
        assert_eq!(
            comments
                .iter()
                .filter(|comment| comment.id == "server-comment-1")
                .count(),
            1
        );
        assert!(app.overlay.is_none());
        assert_eq!(app.toast(), Some("Comment posted"));
    }

    #[tokio::test]
    async fn repeated_submit_button_clicks_are_blocked_while_pending() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (mut app, _, mut receiver) = live_comment_app(calls.clone(), false);
        let submit_hit = (Rect::new(2, 2, 12, 1), 1);
        app.overlay = Some(Overlay::Composer {
            body: "mouse comment".into(),
            error: None,
            retry_requires_refresh: false,
            button_hits: vec![submit_hit],
        });
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 3,
            row: 2,
            modifiers: KeyModifiers::NONE,
        };

        app.handle_mouse(click);
        if let Some(Overlay::Composer { button_hits, .. }) = &mut app.overlay {
            *button_hits = vec![submit_hit];
        }
        app.handle_mouse(click);
        let _ = tokio::time::timeout(std::time::Duration::from_millis(30), receiver.recv()).await;

        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failed_comment_submission_keeps_the_composer_draft() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (mut app, _, mut receiver) = live_comment_app(calls, true);
        app.overlay = Some(Overlay::Composer {
            body: "draft stays here".into(),
            error: None,
            retry_requires_refresh: false,
            button_hits: vec![],
        });

        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL));
        let Some(AppEvent::CommentWrite {
            request,
            correlation_id,
            result,
        }) = receiver.recv().await
        else {
            panic!("comment submit should return a provider result")
        };
        app.apply_comment_write(request, correlation_id, result);

        let Some(Overlay::Composer { body, .. }) = &app.overlay else {
            panic!("failed submission should keep the composer open")
        };
        assert_eq!(body, "draft stays here");
        assert_eq!(
            app.toast.as_deref(),
            Some("Comment failed: permission denied")
        );
    }

    #[tokio::test]
    async fn cancelling_comment_composer_does_not_create_a_toast_or_write() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (mut app, _, mut receiver) = live_comment_app(calls.clone(), false);
        app.overlay = Some(Overlay::Composer {
            body: "draft".into(),
            error: None,
            retry_requires_refresh: false,
            button_hits: vec![],
        });

        app.handle_key(KeyCode::Esc);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), receiver.recv())
                .await
                .is_err()
        );

        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(app.toast.is_none());
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
