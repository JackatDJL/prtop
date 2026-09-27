//! Create pull request / merge request workflow. A staged wizard: repository preflight,
//! field editing with capability-gated pickers, and a final preview. Provider failures
//! preserve every draft field; the submit stage blocks duplicate submissions.

use crate::app::{App, AppEvent};
use crate::editor::TextArea;
use crate::forge::{ForgeCapabilities, ForgeError, NewChangeRequest};
use crate::git::repo::{self, BranchState, PushError, RepoContext};
use crate::model::{ChangeRequest, ChangeRequestKind, Person};
use crate::picker::{PickerItem, PickerKind, PickerSession};
use crate::write::{OpId, WriteState};
use ratatui::layout::Rect;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CreateStage {
    Preflight,
    Fields,
    Preview,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Button {
    Cancel,
    Continue,
    PushAndContinue,
    Create,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PushState {
    Idle,
    WaitingForPush,
    Pushing,
    PushSucceeded,
    PushFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Field {
    Target,
    Title,
    Description,
    Draft,
    Reviewers,
    Labels,
    Assignees,
    Milestone,
}
impl Field {
    pub fn label(self) -> &'static str {
        match self {
            Self::Target => "Target",
            Self::Title => "Title",
            Self::Description => "Description",
            Self::Draft => "Draft",
            Self::Reviewers => "Reviewers",
            Self::Labels => "Labels",
            Self::Assignees => "Assignees",
            Self::Milestone => "Milestone",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CreateEditor {
    Target(PickerSession),
    Title(TextArea),
    Description,
    Reviewers(PickerSession),
    Labels(PickerSession),
    Assignees(PickerSession),
    Milestone(PickerSession),
}

#[derive(Clone, Debug)]
pub struct CreateWorkflow {
    pub forge: String,
    pub host: String,
    pub repository: String,
    pub kind: ChangeRequestKind,
    pub context: RepoContext,
    pub caps: ForgeCapabilities,
    pub demo: bool,
    pub stage: CreateStage,
    pub preflight: Option<BranchState>,
    pub preflight_loading: bool,
    pub preflight_error: Option<String>,
    pub preflight_op: Option<OpId>,
    pub remote_branch_exists: Option<bool>,
    pub pushing: bool,
    pub push_state: PushState,
    pub push_op: Option<OpId>,
    pub push_error: Option<String>,
    pub target: String,
    pub title: String,
    pub body: TextArea,
    pub draft: bool,
    pub reviewers: Vec<Person>,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    pub milestone: Option<String>,
    pub field: usize,
    pub editor: Option<CreateEditor>,
    pub buttons: Vec<Button>,
    pub button_selected: usize,
    pub submit: WriteState<ChangeRequest>,
    pub submit_op: Option<OpId>,
    /// Rects refreshed by the renderer each frame so mouse clicks stay in one state model.
    pub mouse_area: Option<Rect>,
    pub field_hits: Vec<(Rect, usize)>,
    pub button_hits: Vec<(Rect, Button)>,
}

const ALL_FIELDS: [Field; 8] = [
    Field::Target,
    Field::Title,
    Field::Description,
    Field::Draft,
    Field::Reviewers,
    Field::Labels,
    Field::Assignees,
    Field::Milestone,
];

impl CreateWorkflow {
    pub fn new(
        forge: String,
        context: RepoContext,
        kind: ChangeRequestKind,
        caps: ForgeCapabilities,
        demo: bool,
    ) -> Self {
        let repository = context.repository.clone().unwrap_or_default();
        let target = context.default_branch.clone().unwrap_or_default();
        Self {
            forge,
            host: context.host.clone().unwrap_or_default(),
            repository,
            kind,
            caps,
            demo,
            context,
            stage: CreateStage::Preflight,
            preflight: None,
            preflight_loading: true,
            preflight_error: None,
            preflight_op: None,
            remote_branch_exists: None,
            pushing: false,
            push_state: PushState::Idle,
            push_op: None,
            push_error: None,
            target,
            title: String::new(),
            body: TextArea::empty(),
            draft: false,
            reviewers: vec![],
            labels: vec![],
            assignees: vec![],
            milestone: None,
            field: 0,
            editor: None,
            buttons: vec![Button::Cancel, Button::Continue],
            button_selected: 0,
            submit: WriteState::Idle,
            submit_op: None,
            mouse_area: None,
            field_hits: vec![],
            button_hits: vec![],
        }
    }

    pub fn fields(&self) -> Vec<Field> {
        let caps = self.caps;
        ALL_FIELDS
            .into_iter()
            .filter(|field| match field {
                Field::Target | Field::Title | Field::Description => true,
                Field::Draft => caps.create_draft,
                Field::Reviewers => caps.request_reviewers,
                Field::Labels => caps.labels,
                Field::Assignees => caps.assignees,
                Field::Milestone => caps.milestone,
            })
            .collect()
    }
    pub fn current_field(&self) -> Option<Field> {
        self.fields().get(self.field).copied()
    }
    pub(crate) fn set_button(&mut self, buttons: Vec<Button>) {
        if self.buttons != buttons {
            self.buttons = buttons;
            self.button_selected = 0;
        }
    }
    pub fn summary_lines(&self) -> Vec<String> {
        let preflight = self.preflight.clone().unwrap_or_default();
        let commits = match preflight.ahead {
            Some(ahead) => format!("{ahead} ahead"),
            None => "unknown (no local base ref)".into(),
        };
        let behind = match preflight.behind {
            Some(0) | None => String::new(),
            Some(count) => format!(", {count} behind base"),
        };
        let remote = match self.remote_branch_exists {
            Some(true) => format!("{}/{}", self.context.remote, preflight.branch),
            Some(false) => "not pushed".into(),
            None => "checking…".into(),
        };
        vec![
            format!("Repository      {}", self.repository),
            format!("Source          {}", preflight.branch),
            format!("Target          {}", self.target),
            String::new(),
            format!("Commits         {commits}{behind}"),
            format!("Remote          {remote}"),
            format!(
                "Working tree    {}",
                if preflight.dirty {
                    "uncommitted changes"
                } else {
                    "clean"
                }
            ),
            format!(
                "Unpushed        {}",
                preflight
                    .unpushed
                    .map_or_else(|| "unknown".into(), |count| count.to_string())
            ),
            format!(
                "Push            {}",
                match self.push_state {
                    PushState::Idle => "not needed",
                    PushState::WaitingForPush => "required",
                    PushState::Pushing => "in progress",
                    PushState::PushSucceeded => "succeeded",
                    PushState::PushFailed => "failed; retry available",
                }
            ),
        ]
    }
    pub fn start_preflight(&mut self, app: &App) {
        let Some(root) = self.context.root.clone() else {
            // Demo or remote-only project: synthesize the preflight so the flow stays usable.
            self.preflight_loading = false;
            self.preflight = Some(BranchState {
                branch: "feature/new-reader".into(),
                upstream: None,
                remote_branch_exists: Some(false),
                ahead: Some(4),
                behind: Some(0),
                unpushed: Some(4),
                dirty: false,
            });
            self.remote_branch_exists = Some(false);
            self.push_state = PushState::WaitingForPush;
            self.title = "New reader".into();
            self.set_button(vec![Button::Cancel, Button::PushAndContinue]);
            return;
        };
        self.preflight_loading = true;
        self.preflight_error = None;
        let op = OpId::next();
        self.preflight_op = Some(op);
        app.spawn_preflight(
            op,
            root,
            self.context.remote.clone(),
            self.target.clone(),
            self.kind,
        );
    }
    pub fn apply_preflight(
        &mut self,
        state: BranchState,
        remote_exists: Option<bool>,
        template: Option<String>,
        subjects: Vec<String>,
    ) {
        self.preflight_loading = false;
        self.preflight = Some(state.clone());
        if remote_exists.is_some() {
            self.remote_branch_exists = remote_exists;
        }
        self.push_state = if self.push_needed() {
            PushState::WaitingForPush
        } else {
            PushState::Idle
        };
        self.set_button(if self.push_needed() {
            vec![Button::Cancel, Button::PushAndContinue]
        } else {
            vec![Button::Cancel, Button::Continue]
        });
        if self.title.is_empty() {
            self.title = repo::title_from(&state.branch, &subjects, state.ahead.unwrap_or(0));
        }
        if self.body.is_empty()
            && let Some(template) = template
        {
            self.body = TextArea::from_str(&template);
        }
    }
    pub fn push_needed(&self) -> bool {
        self.remote_branch_exists == Some(false)
            || self
                .preflight
                .as_ref()
                .is_some_and(|state| state.unpushed.is_none_or(|count| count > 0))
    }
    pub fn begin_push(&mut self, app: &App) {
        if self.pushing {
            return;
        }
        let op = OpId::next();
        self.push_op = Some(op);
        self.pushing = true;
        self.push_state = PushState::Pushing;
        self.push_error = None;
        let branch = self.preflight_branch();
        let Some(root) = self.context.root.clone() else {
            // Demo push: instant success keeps the flow testable without a repository.
            let sender = app.events.clone();
            tokio::spawn(async move {
                let _ = sender.send(AppEvent::PushCompleted {
                    op,
                    branch,
                    result: Ok(()),
                });
            });
            return;
        };
        let remote = self.context.remote.clone();
        let sender = app.events.clone();
        tokio::spawn(async move {
            let result = repo::push(&root, &remote, &branch).await;
            let branch = repo::current_branch(&root).await.unwrap_or_default();
            let _ = sender.send(AppEvent::PushCompleted { op, branch, result });
        });
    }
    pub fn apply_push(&mut self, op: OpId, branch: String, result: Result<(), PushError>) {
        if self.push_op != Some(op) {
            return;
        }
        self.push_op = None;
        self.pushing = false;
        if result.is_ok() && branch != self.preflight_branch() {
            self.push_error = Some("the checked-out source branch changed during push".into());
            self.push_state = PushState::PushFailed;
            return;
        }
        match result {
            Ok(()) => {
                self.remote_branch_exists = Some(true);
                if let Some(state) = &mut self.preflight {
                    state.remote_branch_exists = Some(true);
                    state.unpushed = Some(0);
                }
                self.push_error = None;
                self.push_state = PushState::PushSucceeded;
                self.stage = CreateStage::Fields;
            }
            Err(PushError::TimedOut) => {
                self.push_error = Some("git push timed out".into());
                self.push_state = PushState::PushFailed;
            }
            Err(PushError::Failed { stderr }) => {
                self.push_error = Some(push_error_summary(&stderr));
                self.push_state = PushState::PushFailed;
            }
        }
    }
    pub fn open_field_editor(&mut self, app: &App, field: Field) {
        match field {
            Field::Target => {
                let token = OpId::next();
                self.editor = Some(CreateEditor::Target(PickerSession::new(
                    PickerKind::TargetBranch,
                    token,
                )));
                if let Some(root) = self.context.root.clone() {
                    app.spawn_target_branches(
                        token,
                        root,
                        self.context.remote.clone(),
                        self.preflight_branch(),
                    );
                } else {
                    let branches = ["main", "develop", "release/1.0"]
                        .into_iter()
                        .filter(|branch| *branch != self.preflight_branch())
                        .map(PickerItem::simple)
                        .collect();
                    if let Some(CreateEditor::Target(session)) = &mut self.editor {
                        session.apply_items(token, branches);
                    }
                }
            }
            Field::Title => {
                self.editor = Some(CreateEditor::Title(TextArea::from_str(&self.title)));
            }
            Field::Description => {
                self.editor = Some(CreateEditor::Description);
            }
            Field::Draft => self.draft = !self.draft,
            Field::Reviewers => {
                let token = OpId::next();
                let mut session = PickerSession::new(PickerKind::Reviewer, token);
                session.checked = self
                    .reviewers
                    .iter()
                    .map(|person| person.login.clone())
                    .collect();
                session.remember_items(self.reviewers.iter().map(|person| PickerItem {
                    id: person.login.clone(),
                    label: person.display_name().to_owned(),
                    detail: person.id.map(|id| id.to_string()),
                }));
                session.multi = true;
                self.editor = Some(CreateEditor::Reviewers(session));
                app.spawn_picker_search(
                    PickerKind::Reviewer,
                    token,
                    &self.forge,
                    &self.repository,
                    "",
                );
            }
            Field::Labels => {
                let token = OpId::next();
                let mut session = PickerSession::new(PickerKind::Label, token);
                session.checked = self.labels.clone();
                self.editor = Some(CreateEditor::Labels(session));
                app.spawn_picker_search(
                    PickerKind::Label,
                    token,
                    &self.forge,
                    &self.repository,
                    "",
                );
            }
            Field::Assignees => {
                let token = OpId::next();
                let mut session = PickerSession::new(PickerKind::Assignee, token);
                session.checked = self.assignees.clone();
                self.editor = Some(CreateEditor::Assignees(session));
                app.spawn_picker_search(
                    PickerKind::Assignee,
                    token,
                    &self.forge,
                    &self.repository,
                    "",
                );
            }
            Field::Milestone => {
                let token = OpId::next();
                self.editor = Some(CreateEditor::Milestone(PickerSession::new(
                    PickerKind::Milestone,
                    token,
                )));
                app.spawn_picker_search(
                    PickerKind::Milestone,
                    token,
                    &self.forge,
                    &self.repository,
                    "",
                );
            }
        }
    }
    fn preflight_branch(&self) -> String {
        self.preflight
            .as_ref()
            .map(|state| state.branch.clone())
            .unwrap_or_default()
    }
    pub fn field_summary(&self, field: Field) -> String {
        match field {
            Field::Target => self.target.clone(),
            Field::Title => {
                if self.title.is_empty() {
                    "(empty)".into()
                } else {
                    self.title.clone()
                }
            }
            Field::Description => {
                let text = self.body.text();
                if text.is_empty() {
                    "(empty)".into()
                } else {
                    let first = text.lines().next().unwrap_or_default().to_owned();
                    format!("{first}…")
                }
            }
            Field::Draft => (if self.draft { "yes" } else { "no" }).into(),
            Field::Reviewers => {
                if self.reviewers.is_empty() {
                    "(none)".into()
                } else {
                    self.reviewers
                        .iter()
                        .map(|person| person.login.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            }
            Field::Labels => {
                if self.labels.is_empty() {
                    "(none)".into()
                } else {
                    self.labels.join(", ")
                }
            }
            Field::Assignees => {
                if self.assignees.is_empty() {
                    "(none)".into()
                } else {
                    self.assignees.join(", ")
                }
            }
            Field::Milestone => self.milestone.clone().unwrap_or_else(|| "(none)".into()),
        }
    }
    pub fn to_input(&self) -> NewChangeRequest {
        NewChangeRequest {
            title: self.title.trim().to_owned(),
            body: self.body.text(),
            source_branch: self.preflight_branch(),
            target_branch: self.target.clone(),
            draft: self.draft,
            reviewers: self.reviewers.iter().map(|p| p.login.clone()).collect(),
            labels: self.labels.clone(),
            assignees: self.assignees.clone(),
            milestone: self.milestone.clone(),
        }
    }
    pub fn begin_submit(&mut self, app: &mut App) {
        if self.submit.is_pending() {
            return;
        }
        if self.remote_branch_exists != Some(true) {
            app.set_toast(if self.push_needed() {
                "Push the source branch before creating the request"
            } else {
                "Waiting for Git preflight"
            });
            return;
        }
        if self.title.trim().is_empty() || self.target.is_empty() {
            app.set_toast("A target branch and title are required");
            return;
        }
        if self.target == self.preflight_branch() {
            app.set_toast("The target branch must differ from the source branch");
            return;
        }
        let op = OpId::next();
        self.submit = WriteState::Pending;
        self.submit_op = Some(op);
        self.stage = CreateStage::Preview;
        if self.demo {
            let input = self.to_input();
            let forge = self.forge.clone();
            let repository = self.repository.clone();
            let kind = self.kind;
            let number = app.next_demo_number(&forge, &repository);
            let sender = app.events.clone();
            tokio::spawn(async move {
                let _ = sender.send(AppEvent::CreateCompleted {
                    op,
                    result: Ok(crate::forge::CreateResult {
                        request: crate::forge::demo::created_request(
                            &forge,
                            &repository,
                            kind,
                            number,
                            &input,
                        ),
                        metadata_warnings: vec![],
                    }),
                });
            });
            return;
        }
        let Some(provider) = app.providers.get(&self.forge).cloned() else {
            self.submit = WriteState::Failed("no provider is configured for this forge".into());
            return;
        };
        let input = self.to_input();
        let repository = self.repository.clone();
        let sender = app.events.clone();
        tokio::spawn(async move {
            let result = provider.create_change_request(&input, &repository).await;
            let _ = sender.send(AppEvent::CreateCompleted { op, result });
        });
    }
    pub fn apply_submit(
        &mut self,
        op: OpId,
        result: Result<ChangeRequest, ForgeError>,
    ) -> Option<ChangeRequest> {
        if self.submit_op != Some(op) {
            return None;
        }
        match result {
            Ok(created) => {
                self.submit = WriteState::Success(created.clone());
                Some(created)
            }
            Err(error) => {
                // The whole draft stays intact so the user can retry or adjust.
                self.submit = WriteState::Failed(crate::app::error_summary(&error));
                None
            }
        }
    }
    pub fn failure_message(&self) -> Option<String> {
        match &self.submit {
            WriteState::Failed(error) => Some(error.clone()),
            _ => None,
        }
    }
    /// Handles one key while the wizard is open. Returns true when the workflow closes.
    pub fn handle_key(
        &mut self,
        app: &mut App,
        key: crossterm::event::KeyCode,
        modifiers: crossterm::event::KeyModifiers,
    ) -> bool {
        use crossterm::event::KeyCode::*;
        if self.stage == CreateStage::Preflight
            && self.preflight_error.is_some()
            && key == Char('r')
        {
            self.start_preflight(app);
            return false;
        }
        if let Some(CreateEditor::Title(_area)) = &mut self.editor {
            match key {
                Esc => {
                    if let Some(CreateEditor::Title(area)) = self.editor.take() {
                        self.title = area.text();
                    }
                }
                Enter if modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
                    if let Some(CreateEditor::Title(area)) = self.editor.take() {
                        self.title = area.text();
                    }
                    self.stage = CreateStage::Preview;
                    self.set_button(vec![Button::Cancel, Button::Create]);
                }
                Enter => {
                    if let Some(CreateEditor::Title(area)) = self.editor.take() {
                        self.title = area.text();
                    }
                    self.field += 1;
                    self.field = self.field.min(self.fields().len().saturating_sub(1));
                }
                Backspace => {
                    if let Some(CreateEditor::Title(area)) = &mut self.editor {
                        area.backspace();
                    }
                }
                Left => {
                    if let Some(CreateEditor::Title(area)) = &mut self.editor {
                        area.left();
                    }
                }
                Right => {
                    if let Some(CreateEditor::Title(area)) = &mut self.editor {
                        area.right();
                    }
                }
                Home => {
                    if let Some(CreateEditor::Title(area)) = &mut self.editor {
                        area.home();
                    }
                }
                End => {
                    if let Some(CreateEditor::Title(area)) = &mut self.editor {
                        area.end();
                    }
                }
                Char(c) => {
                    if let Some(CreateEditor::Title(area)) = &mut self.editor {
                        area.insert_char(c);
                    }
                }
                _ => {}
            }
            return false;
        }
        if let Some(CreateEditor::Description) = &self.editor {
            match key {
                Esc => self.editor = None,
                Enter if modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
                    self.editor = None;
                    self.stage = CreateStage::Preview;
                    self.set_button(vec![Button::Cancel, Button::Create]);
                }
                Enter => self.body.newline(),
                Backspace => self.body.backspace(),
                Up => self.body.up(),
                Down => self.body.down(),
                Left => self.body.left(),
                Right => self.body.right(),
                Home => self.body.home(),
                End => self.body.end(),
                PageUp => {
                    for _ in 0..10 {
                        self.body.up();
                    }
                }
                PageDown => {
                    for _ in 0..10 {
                        self.body.down();
                    }
                }
                Char(c) => self.body.insert_char(c),
                _ => {}
            }
            return false;
        }
        if let Some(editor) = self.editor.take() {
            let picker = match &editor {
                CreateEditor::Target(session)
                | CreateEditor::Reviewers(session)
                | CreateEditor::Labels(session)
                | CreateEditor::Assignees(session)
                | CreateEditor::Milestone(session) => Some(session.clone()),
                _ => None,
            };
            if let Some(mut session) = picker {
                match key {
                    Esc => {
                        self.editor = None;
                        return false;
                    }
                    Backspace => {
                        session.query.pop();
                        session.clamp();
                    }
                    Char(' ') if session.multi => {
                        if let Some(id) = session.selected_item().map(|item| item.id.clone()) {
                            session.toggle_checked(&id);
                        }
                    }
                    Char(c) => {
                        // Re-search live for provider-backed datasets.
                        session.query.push(c);
                        session.clamp();
                        if matches!(session.kind, PickerKind::Reviewer | PickerKind::Assignee) {
                            let token = OpId::next();
                            session.token = token;
                            session.loading = true;
                            self.editor = Some(Self::editor_with_session(&editor, session.clone()));
                            app.spawn_picker_search(
                                session.kind,
                                token,
                                &self.forge,
                                &self.repository,
                                &session.query,
                            );
                            return false;
                        }
                    }
                    Up => session.move_up(),
                    Down => session.move_down(),
                    Enter if modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
                        self.commit_picker(&mut session, true);
                        return false;
                    }
                    Enter => {
                        self.commit_picker(&mut session, false);
                        if session.multi {
                            self.editor = Some(Self::editor_with_session(&editor, session));
                        }
                        return false;
                    }
                    _ => {}
                }
                self.editor = Some(Self::editor_with_session(&editor, session));
                return false;
            }
            self.editor = Some(editor);
        }
        match self.stage {
            CreateStage::Preflight => {
                if self.preflight_loading || self.pushing {
                    return key == Esc;
                }
                if self.push_needed() && !self.pushing {
                    self.set_button(vec![Button::Cancel, Button::PushAndContinue]);
                } else if self.push_needed() {
                    self.set_button(vec![Button::Cancel]);
                } else {
                    self.set_button(vec![Button::Cancel, Button::Continue]);
                }
                match key {
                    Esc => return true,
                    Left | Char('h') => {
                        self.button_selected = self.button_selected.saturating_sub(1)
                    }
                    Right | Char('l') => {
                        self.button_selected =
                            (self.button_selected + 1).min(self.buttons.len() - 1)
                    }
                    Enter => {
                        let button = self.buttons.get(self.button_selected).copied();
                        match button {
                            Some(Button::Cancel) => return true,
                            Some(Button::Continue) => {
                                self.stage = CreateStage::Fields;
                            }
                            Some(Button::PushAndContinue) => self.begin_push(app),
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            CreateStage::Fields => match key {
                Esc => {
                    self.stage = CreateStage::Preflight;
                    self.submit = WriteState::Idle;
                    self.submit_op = None;
                }
                Up | Char('k') => self.field = self.field.saturating_sub(1),
                Down | Char('j') | Tab => {
                    let count = self.fields().len();
                    self.field = (self.field + 1).min(count.saturating_sub(1));
                }
                Enter if modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
                    self.stage = CreateStage::Preview;
                    self.set_button(vec![Button::Cancel, Button::Create]);
                }
                Enter => {
                    if let Some(field) = self.current_field() {
                        self.open_field_editor(app, field);
                    }
                }
                Char(' ') if self.current_field() == Some(Field::Draft) => {
                    self.draft = !self.draft;
                }
                _ => {}
            },
            CreateStage::Preview => {
                if self.submit.is_pending() {
                    return false;
                }
                match key {
                    Esc => {
                        self.stage = CreateStage::Fields;
                        self.submit = WriteState::Idle;
                        self.submit_op = None;
                    }
                    Left | Char('h') => {
                        self.button_selected = self.button_selected.saturating_sub(1)
                    }
                    Right | Char('l') => {
                        self.button_selected =
                            (self.button_selected + 1).min(self.buttons.len() - 1)
                    }
                    Enter if modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
                        self.begin_submit(app);
                    }
                    Enter => {
                        let button = self.buttons.get(self.button_selected).copied();
                        match button {
                            Some(Button::Cancel) => {
                                self.stage = CreateStage::Fields;
                                self.submit = WriteState::Idle;
                                self.submit_op = None;
                            }
                            Some(Button::Create) => self.begin_submit(app),
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
        }
        false
    }
    fn editor_with_session(editor: &CreateEditor, session: PickerSession) -> CreateEditor {
        match editor {
            CreateEditor::Target(_) => CreateEditor::Target(session),
            CreateEditor::Reviewers(_) => CreateEditor::Reviewers(session),
            CreateEditor::Labels(_) => CreateEditor::Labels(session),
            CreateEditor::Assignees(_) => CreateEditor::Assignees(session),
            CreateEditor::Milestone(_) => CreateEditor::Milestone(session),
            CreateEditor::Title(_) | CreateEditor::Description => editor.clone(),
        }
    }
    fn commit_picker(&mut self, session: &mut PickerSession, apply_multi: bool) {
        match session.kind {
            PickerKind::TargetBranch => {
                if let Some(item) = session.selected_item() {
                    self.target = item.id.clone();
                }
                self.editor = None;
            }
            PickerKind::Reviewer => {
                if apply_multi {
                    self.reviewers = session
                        .checked
                        .iter()
                        .map(|login| {
                            session
                                .known_items
                                .iter()
                                .find(|item| item.id == *login)
                                .map(|item| Person {
                                    login: item.id.clone(),
                                    name: item.detail.clone(),
                                    id: item.detail.as_deref().and_then(|id| id.parse().ok()),
                                })
                                .unwrap_or_else(|| Person::named(login.clone()))
                        })
                        .collect();
                    self.editor = None;
                } else if let Some(id) = session.selected_item().map(|item| item.id.clone()) {
                    session.toggle_checked(&id);
                }
            }
            PickerKind::Milestone => {
                if let Some(item) = session.selected_item() {
                    // Selecting the current milestone again clears it.
                    self.milestone = if self.milestone.as_deref() == Some(item.id.as_str()) {
                        None
                    } else {
                        Some(item.id.clone())
                    };
                }
                self.editor = None;
            }
            PickerKind::Label | PickerKind::Assignee => {
                if apply_multi {
                    let checked = session.checked.clone();
                    match session.kind {
                        PickerKind::Label => self.labels = checked,
                        PickerKind::Assignee => self.assignees = checked,
                        _ => {}
                    }
                    self.editor = None;
                } else if let Some(id) = session.selected_item().map(|item| item.id.clone()) {
                    session.toggle_checked(&id);
                }
            }
        }
    }
    /// Handles a mouse click inside the wizard. Rows are hit-tested against the rects the
    /// renderer refreshed this frame.
    pub fn handle_mouse(&mut self, app: &mut App, column: u16, row: u16) -> bool {
        for (rect, button) in self.button_hits.clone() {
            if rect.contains(ratatui::layout::Position { x: column, y: row }) {
                if (self.preflight_loading || self.pushing) && self.stage == CreateStage::Preflight
                {
                    return button == Button::Cancel;
                }
                let picker_open = self.editor.as_ref().is_some_and(|editor| {
                    matches!(
                        editor,
                        CreateEditor::Target(_)
                            | CreateEditor::Reviewers(_)
                            | CreateEditor::Labels(_)
                            | CreateEditor::Assignees(_)
                            | CreateEditor::Milestone(_)
                    )
                });
                if self.stage == CreateStage::Fields && picker_open {
                    match button {
                        Button::Cancel => self.editor = None,
                        Button::Continue => {
                            if let Some(
                                CreateEditor::Target(session)
                                | CreateEditor::Reviewers(session)
                                | CreateEditor::Labels(session)
                                | CreateEditor::Assignees(session)
                                | CreateEditor::Milestone(session),
                            ) = &mut self.editor
                            {
                                let mut session = session.clone();
                                self.commit_picker(&mut session, true);
                            }
                        }
                        _ => {}
                    }
                    return false;
                }
                match button {
                    Button::Cancel => return true,
                    Button::Continue => {
                        if self.stage == CreateStage::Preflight {
                            if self.push_needed() {
                                self.begin_push(app);
                            } else {
                                self.stage = CreateStage::Fields;
                            }
                        } else if self.stage == CreateStage::Fields {
                            if let Some(CreateEditor::Title(area)) = self.editor.take() {
                                self.title = area.text();
                            } else {
                                self.editor = None;
                            }
                            self.stage = CreateStage::Preview;
                            self.set_button(vec![Button::Cancel, Button::Create]);
                        }
                    }
                    Button::PushAndContinue => self.begin_push(app),
                    Button::Create => {
                        if self.stage == CreateStage::Preview {
                            self.begin_submit(app);
                        }
                    }
                }
                return false;
            }
        }
        for (rect, index) in self.field_hits.clone() {
            if rect.contains(ratatui::layout::Position { x: column, y: row })
                && self.stage == CreateStage::Fields
            {
                self.field = index.min(self.fields().len().saturating_sub(1));
                if let Some(field) = self.current_field() {
                    self.open_field_editor(app, field);
                }
                return false;
            }
        }
        if let Some(area) = self.mouse_area
            && area.contains(ratatui::layout::Position { x: column, y: row })
        {
            let row = row.saturating_sub(area.y) as usize;
            if let Some(
                CreateEditor::Target(session)
                | CreateEditor::Reviewers(session)
                | CreateEditor::Labels(session)
                | CreateEditor::Assignees(session)
                | CreateEditor::Milestone(session),
            ) = &self.editor
            {
                let mut session = session.clone();
                if row < session.visible_count() {
                    if row == session.selected {
                        self.commit_picker(&mut session, false);
                        if session.multi
                            && let Some(editor) = &self.editor
                        {
                            self.editor = Some(Self::editor_with_session(editor, session));
                        }
                    } else {
                        session.select_row(row);
                        if let Some(editor) = &self.editor {
                            self.editor = Some(Self::editor_with_session(editor, session));
                        }
                    }
                }
            }
        }
        false
    }
}

fn push_error_summary(stderr: &str) -> String {
    let diagnostic = stderr.to_lowercase();
    if diagnostic.contains("authentication failed")
        || diagnostic.contains("could not read username")
        || diagnostic.contains("permission denied")
    {
        "authentication or permission denied while pushing".into()
    } else if diagnostic.contains("non-fast-forward")
        || diagnostic.contains("fetch first")
        || diagnostic.contains("rejected")
    {
        "remote rejected the push; fetch and reconcile the branch before retrying".into()
    } else if diagnostic.contains("repository not found") || diagnostic.contains("not found") {
        "remote repository not found".into()
    } else {
        "git push failed; check the configured remote and credentials".into()
    }
}

#[cfg(test)]
pub(crate) fn full_caps() -> ForgeCapabilities {
    ForgeCapabilities {
        create_change_request: true,
        create_draft: true,
        edit_title: true,
        edit_description: true,
        labels: true,
        request_reviewers: true,
        assignees: true,
        milestone: true,
        draft_transition: true,
        close: true,
        reopen: true,
        merge: true,
        merge_commit: true,
        squash_merge: true,
        rebase_merge: true,
        auto_merge: true,
        delete_source_branch: true,
        ..ForgeCapabilities::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MergeStrategy;

    fn demo_workflow() -> CreateWorkflow {
        CreateWorkflow::new(
            "github".into(),
            RepoContext {
                root: None,
                remote: "origin".into(),
                host: Some("github.com".into()),
                repository: Some("jack/quickdrop".into()),
                default_branch: Some("main".into()),
            },
            ChangeRequestKind::PullRequest,
            full_caps(),
            true,
        )
    }

    #[test]
    fn fields_are_filtered_by_capabilities() {
        let mut workflow = demo_workflow();
        workflow.caps = ForgeCapabilities {
            create_change_request: true,
            create_draft: true,
            request_reviewers: true,
            ..ForgeCapabilities::default()
        };
        let fields = workflow.fields();
        assert_eq!(
            fields,
            vec![
                Field::Target,
                Field::Title,
                Field::Description,
                Field::Draft,
                Field::Reviewers
            ]
        );
    }

    #[test]
    fn demo_preflight_synthesizes_an_unpushed_branch() {
        let mut workflow = demo_workflow();
        workflow.start_preflight(&App::test_app());
        assert!(!workflow.preflight_loading);
        assert!(workflow.push_needed());
        assert_eq!(workflow.title, "New reader");
    }

    #[test]
    fn unpushed_commits_require_a_push_even_when_remote_branch_exists() {
        let mut workflow = demo_workflow();
        workflow.apply_preflight(
            BranchState {
                branch: "feature/new-reader".into(),
                unpushed: Some(2),
                remote_branch_exists: Some(true),
                ..BranchState::default()
            },
            Some(true),
            None,
            vec![],
        );
        assert!(workflow.push_needed());
        assert_eq!(workflow.push_state, PushState::WaitingForPush);
    }

    #[test]
    fn preflight_escape_can_cancel_while_loading() {
        let mut workflow = demo_workflow();
        workflow.preflight_loading = true;
        let mut app = App::test_app();
        assert!(workflow.handle_key(
            &mut app,
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
    }

    #[test]
    fn preflight_summary_reports_state() {
        let mut workflow = demo_workflow();
        workflow.start_preflight(&App::test_app());
        let lines = workflow.summary_lines().join("\n");
        assert!(lines.contains("jack/quickdrop"));
        assert!(lines.contains("feature/new-reader"));
        assert!(lines.contains("not pushed"));
        assert!(lines.contains("4 ahead"));
    }

    #[test]
    fn push_completion_advances_to_fields() {
        let mut workflow = demo_workflow();
        workflow.start_preflight(&App::test_app());
        let op = OpId::next();
        workflow.push_op = Some(op);
        workflow.apply_push(op, "feature/new-reader".into(), Ok(()));
        assert_eq!(workflow.stage, CreateStage::Fields);
        assert_eq!(workflow.remote_branch_exists, Some(true));
    }

    #[test]
    fn push_failure_keeps_the_preflight_and_reports_the_error() {
        let mut workflow = demo_workflow();
        workflow.start_preflight(&App::test_app());
        let op = OpId::next();
        workflow.push_op = Some(op);
        workflow.apply_push(
            op,
            "feature/new-reader".into(),
            Err(PushError::Failed {
                stderr: "remote: Repository not found".into(),
            }),
        );
        assert_eq!(workflow.stage, CreateStage::Preflight);
        assert_eq!(
            workflow.push_error.as_deref(),
            Some("remote repository not found")
        );
    }

    #[test]
    fn push_completion_detects_a_changed_checkout_branch() {
        let mut workflow = demo_workflow();
        workflow.start_preflight(&App::test_app());
        let op = OpId::next();
        workflow.push_op = Some(op);
        workflow.apply_push(op, "another-branch".into(), Ok(()));
        assert_eq!(workflow.stage, CreateStage::Preflight);
        assert_eq!(
            workflow.push_error.as_deref(),
            Some("the checked-out source branch changed during push")
        );
    }

    #[tokio::test]
    async fn submission_is_blocked_while_a_previous_submit_is_pending() {
        let mut app = App::test_app();
        let mut workflow = demo_workflow();
        workflow.start_preflight(&app);
        let op = OpId::next();
        workflow.push_op = Some(op);
        workflow.apply_push(op, "feature/new-reader".into(), Ok(()));
        workflow.target = "main".into();
        workflow.title = "New reader".into();
        workflow.begin_submit(&mut app);
        assert!(workflow.submit.is_pending());
        let first_op = workflow.submit_op;
        // A second submit attempt (double Enter) must not re-arm the write.
        workflow.begin_submit(&mut app);
        assert_eq!(workflow.submit_op, first_op);
        assert_eq!(app.toast(), None);
    }

    #[tokio::test]
    async fn failed_submission_preserves_the_draft() {
        let mut app = App::test_app();
        let mut workflow = demo_workflow();
        workflow.start_preflight(&app);
        let op = OpId::next();
        workflow.push_op = Some(op);
        workflow.apply_push(op, "feature/new-reader".into(), Ok(()));
        workflow.target = "main".into();
        workflow.title = "New reader".into();
        workflow.body = TextArea::from_str("## Summary\nCareful work.");
        workflow.draft = true;
        workflow.begin_submit(&mut app);
        let op = workflow.submit_op.unwrap();
        let created = workflow
            .apply_submit(
                op,
                Err(ForgeError::Validation("target branch is protected".into())),
            )
            .is_none();
        assert!(created);
        assert_eq!(
            workflow.failure_message().as_deref(),
            Some("validation failed: target branch is protected")
        );
        assert_eq!(workflow.title, "New reader");
        assert!(workflow.draft);
        assert!(workflow.body.text().contains("Careful work."));
    }

    #[tokio::test]
    async fn successful_submission_returns_the_created_request() {
        let mut app = App::test_app();
        let mut workflow = demo_workflow();
        workflow.start_preflight(&app);
        let op = OpId::next();
        workflow.push_op = Some(op);
        workflow.apply_push(op, "feature/new-reader".into(), Ok(()));
        workflow.target = "main".into();
        workflow.title = "New reader".into();
        workflow.begin_submit(&mut app);
        let op = workflow.submit_op.unwrap();
        let created = workflow.apply_submit(
            op,
            Ok(crate::forge::demo::created_request(
                "github",
                "jack/quickdrop",
                ChangeRequestKind::PullRequest,
                app.next_demo_number("github", "jack/quickdrop"),
                &workflow.to_input(),
            )),
        );
        assert!(created.is_some());
        assert_eq!(workflow.stage, CreateStage::Preview);
    }

    #[tokio::test]
    async fn stale_op_results_are_ignored() {
        let mut app = App::test_app();
        let mut workflow = demo_workflow();
        workflow.start_preflight(&app);
        let op = OpId::next();
        workflow.push_op = Some(op);
        workflow.apply_push(op, "feature/new-reader".into(), Ok(()));
        workflow.title = "New reader".into();
        workflow.begin_submit(&mut app);
        let op = workflow.submit_op.unwrap();
        let ignored = workflow
            .apply_submit(
                OpId(op.0 + 99),
                Err(ForgeError::Unavailable("late arrival".into())),
            )
            .is_none();
        assert!(ignored);
        assert!(workflow.submit.is_pending());
    }

    #[test]
    fn picker_commits_update_their_field() {
        let mut workflow = demo_workflow();
        let mut session = PickerSession::new(PickerKind::TargetBranch, OpId(1));
        session.apply_items(
            OpId(1),
            vec![PickerItem::simple("main"), PickerItem::simple("develop")],
        );
        session.selected = 1;
        workflow.commit_picker(&mut session, false);
        assert_eq!(workflow.target, "develop");

        let mut labels = PickerSession::new(PickerKind::Label, OpId(2));
        labels.apply_items(
            OpId(2),
            vec![PickerItem::simple("bug"), PickerItem::simple("mobile")],
        );
        labels.toggle_checked("bug");
        labels.toggle_checked("mobile");
        workflow.commit_picker(&mut labels, true);
        assert_eq!(workflow.labels, vec!["bug", "mobile"]);
    }

    #[test]
    fn enter_toggles_multi_picker_selection_without_losing_the_session() {
        let mut workflow = demo_workflow();
        workflow.stage = CreateStage::Fields;
        let mut app = App::test_app();
        let mut session = PickerSession::new(PickerKind::Reviewer, OpId(9));
        session.apply_items(
            OpId(9),
            vec![PickerItem::simple("alice"), PickerItem::simple("bob")],
        );
        workflow.editor = Some(CreateEditor::Reviewers(session));
        workflow.handle_key(
            &mut app,
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        );
        let Some(CreateEditor::Reviewers(session)) = workflow.editor else {
            panic!("multi picker should remain open after toggling");
        };
        assert_eq!(session.checked, vec!["alice"]);
    }

    #[test]
    fn merge_strategies_still_advertise_from_capabilities() {
        let caps = full_caps();
        let strategies: Vec<MergeStrategy> = [
            (MergeStrategy::Squash, caps.squash_merge),
            (MergeStrategy::MergeCommit, caps.merge_commit),
            (MergeStrategy::Rebase, caps.rebase_merge),
        ]
        .into_iter()
        .filter_map(|(strategy, supported)| supported.then_some(strategy))
        .collect();
        assert_eq!(
            strategies,
            vec![
                MergeStrategy::Squash,
                MergeStrategy::MergeCommit,
                MergeStrategy::Rebase
            ]
        );
    }

    #[test]
    fn input_maps_the_full_form() {
        let mut workflow = demo_workflow();
        workflow.target = "main".into();
        workflow.title = "New reader".into();
        workflow.draft = true;
        workflow.reviewers = vec![Person::named("alice")];
        workflow.labels = vec!["bug".into()];
        workflow.assignees = vec!["bob".into()];
        workflow.milestone = Some("v1.2".into());
        let input = workflow.to_input();
        assert_eq!(input.title, "New reader");
        assert!(input.draft);
        assert_eq!(input.reviewers, vec!["alice"]);
        assert_eq!(input.labels, vec!["bug"]);
        assert_eq!(input.milestone.as_deref(), Some("v1.2"));
    }
}
