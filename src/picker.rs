//! A generic searchable list picker shared by the create wizard and the metadata editor.
//! Keyboard and mouse selection share one state model: `selected` is the single index.

use crate::write::OpId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PickerKind {
    TargetBranch,
    Reviewer,
    Assignee,
    Label,
    Milestone,
}
impl PickerKind {
    pub fn title(self) -> &'static str {
        match self {
            Self::TargetBranch => "Choose target branch",
            Self::Reviewer => "Request a reviewer",
            Self::Assignee => "Choose assignees",
            Self::Label => "Choose labels",
            Self::Milestone => "Choose milestone",
        }
    }
    pub fn multi(self) -> bool {
        matches!(self, Self::Reviewer | Self::Assignee | Self::Label)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PickerItem {
    pub id: String,
    pub label: String,
    pub detail: Option<String>,
}
impl PickerItem {
    pub fn simple(label: impl Into<String>) -> Self {
        let label = label.into();
        Self {
            id: label.clone(),
            label,
            detail: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PickerSession {
    pub kind: PickerKind,
    pub multi: bool,
    pub query: String,
    pub selected: usize,
    pub items: Vec<PickerItem>,
    /// Search results retained so selected people keep their provider IDs across queries.
    pub known_items: Vec<PickerItem>,
    pub checked: Vec<String>,
    /// True after the user deliberately changes the current row.
    pub selection_touched: bool,
    pub loading: bool,
    pub error: Option<String>,
    /// Correlates async loads with the session that requested them.
    pub token: OpId,
}
impl PickerSession {
    pub fn new(kind: PickerKind, token: OpId) -> Self {
        Self {
            multi: kind.multi(),
            kind,
            query: String::new(),
            selected: 0,
            items: vec![],
            known_items: vec![],
            checked: vec![],
            selection_touched: false,
            loading: true,
            error: None,
            token,
        }
    }
    pub fn filtered(&self) -> Vec<&PickerItem> {
        let query = self.query.to_lowercase();
        self.items
            .iter()
            .filter(|item| {
                query.is_empty()
                    || item.label.to_lowercase().contains(&query)
                    || item.id.to_lowercase().contains(&query)
            })
            .collect()
    }
    pub fn visible_count(&self) -> usize {
        self.filtered().len()
    }
    pub fn clamp(&mut self) {
        self.selected = self.selected.min(self.visible_count().saturating_sub(1));
    }
    pub fn move_down(&mut self) {
        let selected = (self.selected + 1).min(self.visible_count().saturating_sub(1));
        self.selection_touched = true;
        self.selected = selected;
    }
    pub fn move_up(&mut self) {
        let selected = self.selected.saturating_sub(1);
        self.selection_touched = true;
        self.selected = selected;
    }
    pub fn select_row(&mut self, row: usize) {
        let selected = row.min(self.visible_count().saturating_sub(1));
        self.selection_touched = true;
        self.selected = selected;
    }
    pub fn selected_item(&self) -> Option<&PickerItem> {
        self.filtered().get(self.selected).copied()
    }
    pub fn toggle_checked(&mut self, id: &str) {
        if !self.multi {
            return;
        }
        if let Some(position) = self.checked.iter().position(|checked| checked == id) {
            self.checked.remove(position);
        } else {
            self.checked.push(id.to_owned());
        }
        self.selection_touched = true;
    }
    pub fn is_checked(&self, id: &str) -> bool {
        self.checked.iter().any(|checked| checked == id)
    }
    pub fn apply_items(&mut self, token: OpId, items: Vec<PickerItem>) {
        if token == self.token {
            self.remember_items(items.iter().cloned());
            self.items = items;
            self.loading = false;
            if !self.selection_touched
                && let Some(selected) = self
                    .items
                    .iter()
                    .position(|item| self.checked.iter().any(|id| id == &item.id))
            {
                self.selected = selected;
            }
            self.clamp();
        }
    }
    pub fn remember_items(&mut self, items: impl IntoIterator<Item = PickerItem>) {
        for item in items {
            if let Some(existing) = self
                .known_items
                .iter_mut()
                .find(|known| known.id == item.id)
            {
                *existing = item;
            } else {
                self.known_items.push(item);
            }
        }
    }
    pub fn failed(&mut self, token: OpId, error: String) {
        if token == self.token {
            self.loading = false;
            self.error = Some(error);
        }
    }
    pub fn status_line(&self) -> String {
        if let Some(error) = &self.error {
            return format!("error: {error}");
        }
        if self.loading {
            return "loading…".into();
        }
        if self.multi && !self.checked.is_empty() {
            return format!("selected: {}", self.checked.join(", "));
        }
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> PickerSession {
        let mut session = PickerSession::new(PickerKind::TargetBranch, OpId(1));
        session.loading = false;
        session.items = vec![
            PickerItem::simple("main"),
            PickerItem::simple("develop"),
            PickerItem::simple("release/1.0"),
        ];
        session
    }

    #[test]
    fn filtering_narrows_and_clamps_selection() {
        let mut session = session();
        session.query = "re".into();
        assert_eq!(session.visible_count(), 1);
        session.clamp();
        assert_eq!(session.selected, 0);
        assert_eq!(session.selected_item().unwrap().label, "release/1.0");
    }

    #[test]
    fn multi_selection_toggles_in_order() {
        let mut session = session();
        session.kind = PickerKind::Label;
        session.multi = true;
        session.toggle_checked("main");
        session.toggle_checked("develop");
        session.toggle_checked("main");
        assert_eq!(session.checked, vec!["develop"]);
    }

    #[test]
    fn single_kind_never_toggles() {
        let mut session = session();
        session.toggle_checked("main");
        assert!(session.checked.is_empty());
    }

    #[test]
    fn stale_tokens_do_not_update_a_newer_session() {
        let mut session = session();
        session.apply_items(OpId(2), vec![PickerItem::simple("late")]);
        assert_eq!(session.items.len(), 3);
    }

    #[test]
    fn selected_people_keep_provider_ids_across_searches() {
        let mut session = PickerSession::new(PickerKind::Reviewer, OpId(1));
        session.apply_items(
            OpId(1),
            vec![PickerItem {
                id: "bob".into(),
                label: "Bob Example".into(),
                detail: Some("42".into()),
            }],
        );
        session.toggle_checked("bob");
        session.token = OpId(2);
        session.query = "ali".into();
        session.loading = true;
        session.apply_items(
            OpId(2),
            vec![PickerItem {
                id: "alice".into(),
                label: "Alice Example".into(),
                detail: Some("7".into()),
            }],
        );

        assert_eq!(session.checked, vec!["bob".to_owned()]);
        assert_eq!(
            session
                .known_items
                .iter()
                .find(|item| item.id == "bob")
                .unwrap()
                .detail
                .as_deref(),
            Some("42")
        );
    }

    #[test]
    fn loaded_items_select_the_current_value_without_marking_it_touched() {
        let mut session = PickerSession::new(PickerKind::Milestone, OpId(1));
        session.checked.push("v1.2".into());
        session.apply_items(
            OpId(1),
            vec![PickerItem::simple("v1.3"), PickerItem::simple("v1.2")],
        );
        assert_eq!(session.selected_item().unwrap().id, "v1.2");
        assert!(!session.selection_touched);
    }
}
