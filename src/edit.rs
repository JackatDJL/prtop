//! In-place editing of a change request's title, body, and metadata. Every write is
//! capability-gated and runs asynchronously; a Pending write blocks resubmission.

use crate::app::{App, ConfirmAction, ConfirmDialog, LifecycleAction, Overlay};
use crate::editor::TextArea;
#[cfg(test)]
use crate::forge::ForgeError;
use crate::forge::{ForgeCapabilities, RequestPatch};
use crate::model::{ChangeRequest, ChangeRequestId, Person, RequestState};
use crate::picker::{PickerKind, PickerSession};
use crate::write::OpId;
use ratatui::layout::Rect;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditField {
    Title(TextArea),
    Body(TextArea),
    Picker(PickerSession),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditAction {
    Title,
    Description,
    Labels,
    Reviewers,
    Assignees,
    Milestone,
    Draft,
    Ready,
    Close,
    Reopen,
    DeleteRemoteBranch,
}

#[derive(Clone, Debug)]
pub struct EditMenuItem {
    pub name: String,
    pub action: EditAction,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditSession {
    pub id: ChangeRequestId,
    pub menu_selected: usize,
    pub active: Option<EditField>,
    pub pending: Option<(OpId, String)>,
    pub caps: ForgeCapabilities,
    pub area: Option<Rect>,
    pub item_hits: Vec<(Rect, usize)>,
    pub mouse_area: Option<Rect>,
    pub button_hits: Vec<(Rect, usize)>,
}
impl EditSession {
    pub fn new(id: ChangeRequestId, caps: ForgeCapabilities) -> Self {
        Self {
            id,
            menu_selected: 0,
            active: None,
            pending: None,
            caps,
            area: None,
            item_hits: vec![],
            mouse_area: None,
            button_hits: vec![],
        }
    }
    pub fn menu_items(&self, request: &ChangeRequest) -> Vec<EditMenuItem> {
        let caps = self.caps;
        let open = request.state == RequestState::Open;
        let mut items = vec![];
        let mut push = |name: &str, action: EditAction, reason: Option<String>| {
            items.push(EditMenuItem {
                name: name.into(),
                action,
                reason,
            })
        };
        push(
            "Edit title",
            EditAction::Title,
            (!caps.edit_title)
                .then(|| "This forge has not advertised this write capability".into()),
        );
        push(
            "Edit description",
            EditAction::Description,
            (!caps.edit_description)
                .then(|| "This forge has not advertised this write capability".into()),
        );
        push(
            "Edit labels",
            EditAction::Labels,
            (!caps.labels).then(|| "This forge does not support labels".into()),
        );
        push(
            "Edit reviewers",
            EditAction::Reviewers,
            (!caps.request_reviewers)
                .then(|| "This forge does not support reviewer requests".into()),
        );
        push(
            "Edit assignees",
            EditAction::Assignees,
            (!caps.assignees).then(|| "This forge does not support assignees".into()),
        );
        push(
            "Edit milestone",
            EditAction::Milestone,
            (!caps.milestone).then(|| "This forge does not support milestones".into()),
        );
        if request.draft {
            push(
                "Mark ready for review",
                EditAction::Ready,
                (!caps.draft_transition)
                    .then(|| "This forge cannot transition drafts through its API".into()),
            );
        } else if open {
            push(
                "Mark as draft",
                EditAction::Draft,
                (!caps.draft_transition)
                    .then(|| "This forge cannot transition drafts through its API".into()),
            );
        }
        if open {
            push(
                "Close",
                EditAction::Close,
                (!caps.close).then(|| "This forge has not advertised this write capability".into()),
            );
        } else {
            push(
                "Reopen",
                EditAction::Reopen,
                (!caps.reopen)
                    .then(|| "This forge has not advertised this write capability".into()),
            );
        }
        push(
            "Delete remote source branch",
            EditAction::DeleteRemoteBranch,
            (!caps.delete_source_branch)
                .then(|| "This forge has not advertised this write capability".into()),
        );
        items
            .into_iter()
            .filter(|item| item.reason.is_none())
            .collect()
    }
    pub fn current_item(&self, request: &ChangeRequest) -> Option<EditMenuItem> {
        self.menu_items(request).get(self.menu_selected).cloned()
    }
    pub fn menu_len(&self, request: &ChangeRequest) -> usize {
        self.menu_items(request).len()
    }

    /// Returns true when the session should close.
    pub fn handle_key(
        &mut self,
        app: &mut App,
        key: crossterm::event::KeyCode,
        modifiers: crossterm::event::KeyModifiers,
    ) -> bool {
        use crossterm::event::KeyCode::*;
        use crossterm::event::KeyModifiers;
        if self.pending.is_some() {
            return false;
        }
        if let Some(EditField::Title(area)) = &mut self.active {
            match key {
                Esc => self.active = None,
                Enter if modifiers.contains(KeyModifiers::CONTROL) => {
                    let title = match &self.active {
                        Some(EditField::Title(area)) => area.text().trim().to_owned(),
                        _ => String::new(),
                    };
                    app.start_update(
                        self.id.clone(),
                        RequestPatch {
                            title: Some(title),
                            ..RequestPatch::default()
                        },
                        LifecycleAction::Title,
                    );
                    self.pending = app.last_op.map(|op| (op, "Updating title".into()));
                }
                Enter => {}
                Backspace => area.backspace(),
                Left => area.left(),
                Right => area.right(),
                Home => area.home(),
                End => area.end(),
                Char(c) => area.insert_char(c),
                _ => {}
            }
            return false;
        }
        if let Some(EditField::Body(_)) = &self.active {
            match key {
                Esc => self.active = None,
                Enter if modifiers.contains(KeyModifiers::CONTROL) => {
                    let body = match &self.active {
                        Some(EditField::Body(area)) => area.text(),
                        _ => String::new(),
                    };
                    app.start_update(
                        self.id.clone(),
                        RequestPatch {
                            body: Some(body),
                            ..RequestPatch::default()
                        },
                        LifecycleAction::Body,
                    );
                    self.pending = app.last_op.map(|op| (op, "Updating description".into()));
                }
                Enter => {
                    if let Some(EditField::Body(area)) = &mut self.active {
                        area.newline();
                    }
                }
                Backspace => {
                    if let Some(EditField::Body(area)) = &mut self.active {
                        area.backspace();
                    }
                }
                Up => {
                    if let Some(EditField::Body(area)) = &mut self.active {
                        area.up();
                    }
                }
                Down => {
                    if let Some(EditField::Body(area)) = &mut self.active {
                        area.down();
                    }
                }
                Left => {
                    if let Some(EditField::Body(area)) = &mut self.active {
                        area.left();
                    }
                }
                Right => {
                    if let Some(EditField::Body(area)) = &mut self.active {
                        area.right();
                    }
                }
                Home => {
                    if let Some(EditField::Body(area)) = &mut self.active {
                        area.home();
                    }
                }
                End => {
                    if let Some(EditField::Body(area)) = &mut self.active {
                        area.end();
                    }
                }
                Char(c) => {
                    if let Some(EditField::Body(area)) = &mut self.active {
                        area.insert_char(c);
                    }
                }
                _ => {}
            }
            return false;
        }
        if let Some(EditField::Picker(session)) = &mut self.active {
            let mut session = session.clone();
            match key {
                Esc => self.active = None,
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
                    session.query.push(c);
                    session.clamp();
                    if matches!(session.kind, PickerKind::Reviewer | PickerKind::Assignee) {
                        let token = OpId::next();
                        session.token = token;
                        session.loading = true;
                        let query = session.query.clone();
                        let kind = session.kind;
                        self.active = Some(EditField::Picker(session));
                        app.spawn_picker_search(
                            kind,
                            token,
                            &self.id.forge,
                            &self.id.repository,
                            &query,
                        );
                        return false;
                    }
                }
                Up => session.move_up(),
                Down => session.move_down(),
                Enter if modifiers.contains(KeyModifiers::CONTROL) => {
                    self.commit_picker(app, &mut session, true);
                }
                Enter => self.commit_picker(app, &mut session, false),
                _ => {}
            }
            if let Some(EditField::Picker(current)) = &mut self.active {
                *current = session;
            }
            return false;
        }
        match key {
            Esc => return true,
            Up | Char('k') => self.menu_selected = self.menu_selected.saturating_sub(1),
            Down | Char('j') => {
                let len = app
                    .detail_request()
                    .or_else(|| app.request_for_view())
                    .map(|request| self.menu_len(request))
                    .unwrap_or(1);
                self.menu_selected = (self.menu_selected + 1).min(len.saturating_sub(1));
            }
            Enter => {
                let action = app
                    .detail_request()
                    .or_else(|| app.request_for_view())
                    .and_then(|request| self.current_item(request))
                    .filter(|item| item.reason.is_none())
                    .map(|item| item.action);
                if let Some(action) = action {
                    self.dispatch(app, action);
                }
            }
            _ => {}
        }
        false
    }

    pub(crate) fn dispatch(&mut self, app: &mut App, action: EditAction) {
        let Some(request) = app
            .detail_request()
            .or_else(|| app.request_for_view())
            .cloned()
        else {
            return;
        };
        match action {
            EditAction::Title => {
                self.active = Some(EditField::Title(TextArea::from_str(&request.title)));
            }
            EditAction::Description => {
                self.active = Some(EditField::Body(TextArea::from_str(
                    request.body.as_deref().unwrap_or(""),
                )));
            }
            EditAction::Labels => {
                let mut session = PickerSession::new(PickerKind::Label, OpId::next());
                session.checked = request
                    .labels
                    .iter()
                    .map(|label| label.name.clone())
                    .collect();
                self.active = Some(EditField::Picker(session));
                self.load_picker(app, PickerKind::Label, "");
            }
            EditAction::Reviewers => {
                let mut session = PickerSession::new(PickerKind::Reviewer, OpId::next());
                session.multi = true;
                session.checked = request
                    .reviewers
                    .iter()
                    .filter(|reviewer| reviewer.state == crate::model::ReviewState::Requested)
                    .map(|reviewer| reviewer.person.login.clone())
                    .collect();
                self.active = Some(EditField::Picker(session));
                self.load_picker(app, PickerKind::Reviewer, "");
            }
            EditAction::Assignees => {
                let mut session = PickerSession::new(PickerKind::Assignee, OpId::next());
                session.checked = request
                    .assignees
                    .iter()
                    .map(|person| person.login.clone())
                    .collect();
                self.active = Some(EditField::Picker(session));
                self.load_picker(app, PickerKind::Assignee, "");
            }
            EditAction::Milestone => {
                let mut session = PickerSession::new(PickerKind::Milestone, OpId::next());
                if let Some(milestone) = &request.milestone {
                    session.checked.push(milestone.clone());
                }
                self.active = Some(EditField::Picker(session));
                self.load_picker(app, PickerKind::Milestone, "");
            }
            EditAction::Draft => {
                app.start_update(
                    self.id.clone(),
                    RequestPatch {
                        draft: Some(true),
                        ..RequestPatch::default()
                    },
                    LifecycleAction::Draft,
                );
                self.pending = app.last_op.map(|op| (op, "Marking draft".into()));
            }
            EditAction::Ready => {
                app.start_update(
                    self.id.clone(),
                    RequestPatch {
                        draft: Some(false),
                        ..RequestPatch::default()
                    },
                    LifecycleAction::Ready,
                );
                self.pending = app.last_op.map(|op| (op, "Marking ready".into()));
            }
            EditAction::Close => {
                app.overlay = Some(Overlay::Confirm(ConfirmDialog {
                    title: format!("Close {}?", self.id.display(request.kind)),
                    body: format!(
                        "This will close {} without merging it.",
                        self.id.display(request.kind)
                    ),
                    confirm: "Close".into(),
                    cancel: "Keep open".into(),
                    danger: false,
                    selected: 0,
                    action: ConfirmAction::CloseRequest(self.id.clone()),
                    button_hits: vec![],
                }));
            }
            EditAction::Reopen => {
                app.start_update(
                    self.id.clone(),
                    RequestPatch {
                        state: Some(RequestState::Open),
                        ..RequestPatch::default()
                    },
                    LifecycleAction::Reopen,
                );
                self.pending = app.last_op.map(|op| (op, "Reopening".into()));
            }
            EditAction::DeleteRemoteBranch => {
                app.overlay = Some(Overlay::Confirm(ConfirmDialog {
                    title: "Delete remote branch?".into(),
                    body: format!(
                        "Delete {} on the forge? This cannot be undone from prtop.",
                        request.source_branch
                    ),
                    confirm: "Delete".into(),
                    cancel: "Keep branch".into(),
                    danger: true,
                    selected: 0,
                    action: ConfirmAction::DeleteRemoteBranch {
                        id: self.id.clone(),
                        branch: request.source_branch.clone(),
                    },
                    button_hits: vec![],
                }));
            }
        }
    }
    fn load_picker(&mut self, app: &mut App, kind: PickerKind, query: &str) {
        let token = match &self.active {
            Some(EditField::Picker(session)) => session.token,
            _ => return,
        };
        app.spawn_picker_search(kind, token, &self.id.forge, &self.id.repository, query);
    }
    fn commit_picker(&mut self, app: &mut App, session: &mut PickerSession, apply_multi: bool) {
        match session.kind {
            PickerKind::Reviewer => {
                if apply_multi {
                    let requested = session.checked.clone();
                    let current: Vec<String> = app
                        .detail_request()
                        .or_else(|| app.request_for_view())
                        .map(|request| {
                            request
                                .reviewers
                                .iter()
                                .filter(|reviewer| {
                                    reviewer.state == crate::model::ReviewState::Requested
                                })
                                .map(|reviewer| reviewer.person.login.clone())
                                .collect()
                        })
                        .unwrap_or_default();
                    let add: Vec<Person> = requested
                        .iter()
                        .filter(|login| !current.contains(login))
                        .map(|login| {
                            let id = session
                                .items
                                .iter()
                                .find(|item| item.id == **login)
                                .and_then(|item| item.detail.as_deref())
                                .and_then(|id| id.parse().ok());
                            match id {
                                Some(id) => Person::with_id(login.clone(), id),
                                None => Person::named(login.clone()),
                            }
                        })
                        .collect();
                    let remove: Vec<String> = current
                        .iter()
                        .filter(|login| !requested.contains(login))
                        .cloned()
                        .collect();
                    if add.is_empty() && remove.is_empty() {
                        self.active = None;
                        return;
                    }
                    app.start_reviewers(self.id.clone(), add, remove);
                    self.pending = app.last_op.map(|op| (op, "Updating reviewers".into()));
                    self.active = Some(EditField::Picker(session.clone()));
                } else if let Some(id) = session.selected_item().map(|item| item.id.clone()) {
                    session.toggle_checked(&id);
                }
            }
            PickerKind::Label => {
                if apply_multi {
                    let checked = session.checked.clone();
                    app.start_labels(self.id.clone(), checked);
                    self.pending = app.last_op.map(|op| (op, "Updating labels".into()));
                    self.active = Some(EditField::Picker(session.clone()));
                } else if let Some(id) = session.selected_item().map(|item| item.id.clone()) {
                    session.toggle_checked(&id);
                }
            }
            PickerKind::Assignee => {
                if apply_multi {
                    let checked = session.checked.clone();
                    app.start_assignees(self.id.clone(), checked);
                    self.pending = app.last_op.map(|op| (op, "Updating assignees".into()));
                    self.active = Some(EditField::Picker(session.clone()));
                } else if let Some(id) = session.selected_item().map(|item| item.id.clone()) {
                    session.toggle_checked(&id);
                }
            }
            PickerKind::Milestone => {
                if let Some(item) = session.selected_item() {
                    let current = app
                        .detail_request()
                        .or_else(|| app.request_for_view())
                        .and_then(|request| request.milestone.clone());
                    let next = if current.as_deref() == Some(item.id.as_str()) {
                        None
                    } else {
                        Some(item.id.clone())
                    };
                    app.start_milestone(self.id.clone(), next);
                    self.pending = app.last_op.map(|op| (op, "Updating milestone".into()));
                }
                self.active = Some(EditField::Picker(session.clone()));
            }
            PickerKind::TargetBranch => self.active = None,
        }
    }
    pub fn handle_mouse(&mut self, app: &mut App, column: u16, row: u16) {
        if self.pending.is_some() {
            return;
        }
        let position = ratatui::layout::Position { x: column, y: row };
        if let Some(EditField::Picker(mut session)) = self.active.clone() {
            for (rect, index) in self.button_hits.clone() {
                if rect.contains(position) {
                    if index == 0 {
                        self.active = None;
                    } else {
                        self.commit_picker(app, &mut session, true);
                    }
                    return;
                }
            }
        }
        for (rect, index) in self.item_hits.clone() {
            if rect.contains(position) {
                self.menu_selected = index;
                if let Some(request) = app.detail_request().or_else(|| app.request_for_view())
                    && self
                        .menu_items(request)
                        .get(index)
                        .is_some_and(|item| item.reason.is_none())
                {
                    let action = self.menu_items(request)[index].action.clone();
                    self.dispatch(app, action);
                }
                return;
            }
        }
        for (rect, index) in self.button_hits.clone() {
            if rect.contains(position) {
                self.menu_selected = index;
                if let Some(request) = app.detail_request().or_else(|| app.request_for_view())
                    && self
                        .menu_items(request)
                        .get(index)
                        .is_some_and(|item| item.reason.is_none())
                {
                    let action = self.menu_items(request)[index].action.clone();
                    self.dispatch(app, action);
                }
                return;
            }
        }
        if let Some(area) = self.mouse_area
            && area.contains(position)
        {
            let relative = row.saturating_sub(area.y) as usize;
            if let Some(EditField::Picker(session)) = &self.active {
                let mut session = session.clone();
                if relative < session.visible_count() {
                    if relative == session.selected {
                        self.commit_picker(app, &mut session, false);
                    } else {
                        session.select_row(relative);
                        if let Some(EditField::Picker(current)) = &mut self.active {
                            *current = session;
                        }
                    }
                }
            }
        }
    }
    pub fn pending_label(&self) -> Option<&str> {
        self.pending.as_ref().map(|(_, label)| label.as_str())
    }
}

/// Applies a metadata result to the normalized request without losing locally cached
/// comments, pipelines, or review state.
pub(crate) fn apply_metadata_payload(request: &mut ChangeRequest, payload: &MetaPayload) {
    if !payload.labels.is_empty() || payload.labels_set {
        request.labels = payload.labels.clone();
    }
    if !payload.assignees.is_empty() || payload.assignees_set {
        request.assignees = payload.assignees.clone();
    }
    if payload.milestone_set {
        request.milestone = payload.milestone.clone();
    }
    if payload.reviewers_set {
        request.reviewers = payload.reviewers.clone();
    }
}

#[derive(Clone, Debug, Default)]
pub struct MetaPayload {
    pub labels: Vec<crate::model::Label>,
    pub labels_set: bool,
    pub assignees: Vec<Person>,
    pub assignees_set: bool,
    pub reviewers: Vec<crate::model::Reviewer>,
    pub reviewers_set: bool,
    pub milestone: Option<String>,
    pub milestone_set: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::create::full_caps;
    use crate::forge::demo;
    use crossterm::event::{KeyCode, KeyModifiers};

    fn open_edit_menu() -> (App, ChangeRequestId) {
        let mut app = App::test_app();
        let id = app.requests[0].id.clone();
        app.overlay = Some(Overlay::Edit(EditSession::new(id.clone(), full_caps())));
        (app, id)
    }

    #[test]
    fn menu_is_capability_gated() {
        let app = App::test_app();
        let request = app.requests[0].clone();
        let caps = ForgeCapabilities {
            edit_title: true,
            edit_description: true,
            labels: true,
            ..ForgeCapabilities::default()
        };
        let session = EditSession::new(request.id.clone(), caps);
        let items = session.menu_items(&request);
        let names: Vec<String> = items.into_iter().map(|item| item.name).collect();
        assert!(names.iter().any(|name| name == "Edit title"));
        assert!(names.iter().any(|name| name == "Edit labels"));
        assert!(!names.iter().any(|name| name == "Mark as draft"));
        assert!(!names.iter().any(|name| name == "Edit reviewers"));
        let draft_only = ForgeCapabilities {
            edit_title: true,
            draft_transition: true,
            close: true,
            ..ForgeCapabilities::default()
        };
        let session = EditSession::new(request.id.clone(), draft_only);
        let items = session.menu_items(&request);
        let names: Vec<String> = items.into_iter().map(|item| item.name).collect();
        assert!(names.iter().any(|name| name == "Mark as draft"));
        assert!(names.iter().any(|name| name == "Close"));
        assert!(!names.iter().any(|name| name == "Edit labels"));
    }

    #[test]
    fn close_requires_confirmation_with_keep_open_default() {
        let (mut app, id) = open_edit_menu();
        app.selected = 0;
        app.handle_key(KeyCode::Enter);
        // The first menu entry is "Edit title"; navigate to Close.
        app.overlay = Some(Overlay::Edit(EditSession::new(id.clone(), full_caps())));
        if let Some(Overlay::Edit(session)) = &mut app.overlay {
            session.menu_selected = 7;
        }
        app.handle_key(KeyCode::Enter);
        match &app.overlay {
            Some(Overlay::Confirm(dialog)) => {
                assert!(matches!(dialog.action, ConfirmAction::CloseRequest(_)));
                assert_eq!(dialog.selected, 0);
                assert_eq!(dialog.cancel, "Keep open");
            }
            other => panic!("expected a confirm dialog, got {other:?}"),
        }
        // Enter on the safe default closes the dialog without writing.
        app.handle_key(KeyCode::Enter);
        assert!(app.overlay.is_none());
        assert!(app.in_flight.is_empty());
    }

    #[tokio::test]
    async fn title_editing_submits_through_the_provider() {
        let (mut app, id) = open_edit_menu();
        app.selected = 0;
        app.demo = false;
        app.providers.insert(
            id.forge.clone(),
            std::sync::Arc::new(RecordingProvider::default()),
        );
        let Some(Overlay::Edit(mut session)) = app.overlay.take() else {
            panic!("expected edit overlay");
        };
        session.dispatch(&mut app, EditAction::Title);
        if let Some(EditField::Title(area)) = &mut session.active {
            for c in "Renamed".chars() {
                area.insert_char(c);
            }
        }
        session.handle_key(&mut app, KeyCode::Enter, KeyModifiers::CONTROL);
        app.overlay = Some(Overlay::Edit(session));
        assert!(app.in_flight.contains_key(&(id, "update")));
    }

    #[tokio::test]
    async fn pending_write_blocks_second_submission() {
        let (mut app, id) = open_edit_menu();
        app.demo = false;
        app.providers.insert(
            id.forge.clone(),
            std::sync::Arc::new(RecordingProvider::default()),
        );
        let patch = RequestPatch {
            draft: Some(true),
            ..RequestPatch::default()
        };
        app.start_update(id.clone(), patch.clone(), LifecycleAction::Draft);
        let claimed = app.claim(&id, "update");
        assert!(claimed.is_none());
        assert!(app.in_flight.contains_key(&(id, "update")));
    }

    #[test]
    fn capability_gating_blocks_the_demo_provider_where_it_should() {
        let caps = demo::capabilities_for("codeberg");
        assert!(!caps.squash_merge);
        assert!(!caps.auto_merge);
        assert!(caps.merge && caps.close && caps.labels);
        let caps = demo::capabilities_for("volt-gitlab");
        assert!(!caps.rebase_merge);
        assert!(!caps.request_changes);
        assert!(caps.squash_merge);
    }

    #[test]
    fn payload_application_keeps_local_review_state() {
        let app = App::test_app();
        let mut request = app.requests[0].clone();
        let before = request.review;
        let payload = MetaPayload {
            labels: vec![crate::model::Label::named("mobile")],
            labels_set: true,
            ..MetaPayload::default()
        };
        apply_metadata_payload(&mut request, &payload);
        assert_eq!(request.labels.len(), 1);
        assert_eq!(request.review, before);
    }

    #[tokio::test]
    async fn edit_menu_dispatches_labels_picker_load() {
        let (mut app, id) = open_edit_menu();
        app.demo = false;
        app.providers.insert(
            id.forge.clone(),
            std::sync::Arc::new(RecordingProvider::default()),
        );
        let Some(Overlay::Edit(mut session)) = app.overlay.take() else {
            panic!("expected edit overlay");
        };
        session.dispatch(&mut app, EditAction::Labels);
        app.overlay = Some(Overlay::Edit(session));
        match &app.overlay {
            Some(Overlay::Edit(session)) => {
                assert!(matches!(session.active, Some(EditField::Picker(_))));
            }
            other => panic!("expected an edit session, got {other:?}"),
        }
    }
}

/// A provider that records calls without network access, used to assert dispatch wiring.
#[derive(Default)]
#[cfg(test)]
pub struct RecordingProvider {
    calls: std::sync::atomic::AtomicUsize,
}
#[cfg(test)]
#[async_trait::async_trait]
impl crate::forge::ForgeProvider for RecordingProvider {
    fn name(&self) -> &str {
        "recording"
    }
    fn capabilities(&self) -> ForgeCapabilities {
        crate::create::full_caps()
    }
    async fn list_change_requests(&self) -> Result<Vec<ChangeRequest>, ForgeError> {
        Ok(vec![])
    }
    async fn update_change_request(
        &self,
        _id: &ChangeRequestId,
        _patch: &RequestPatch,
    ) -> Result<ChangeRequest, ForgeError> {
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Err(ForgeError::Unavailable("offline test".into()))
    }
    async fn set_labels(
        &self,
        _id: &ChangeRequestId,
        _names: &[String],
    ) -> Result<Vec<crate::model::Label>, ForgeError> {
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Err(ForgeError::Unavailable("offline test".into()))
    }
    async fn set_assignees(
        &self,
        _id: &ChangeRequestId,
        _logins: &[String],
    ) -> Result<Vec<Person>, ForgeError> {
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Err(ForgeError::Unavailable("offline test".into()))
    }
    async fn set_milestone(
        &self,
        _id: &ChangeRequestId,
        _milestone: Option<&str>,
    ) -> Result<Option<String>, ForgeError> {
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Err(ForgeError::Unavailable("offline test".into()))
    }
    async fn request_reviewer(
        &self,
        _id: &ChangeRequestId,
        _reviewer: &Person,
    ) -> Result<(), ForgeError> {
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Err(ForgeError::Unavailable("offline test".into()))
    }
}
