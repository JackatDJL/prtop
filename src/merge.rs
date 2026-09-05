//! Merge preflight, strategy selection, and confirmations. Strategy availability comes from
//! provider capabilities; policy warnings never silently turn into a merge.

use crate::app::{App, AppEvent, Overlay};
use crate::forge::ForgeCapabilities;
use crate::model::{ChangeRequest, ChangeRequestId, CiState, MergeOutcome, MergeStrategy, Mergeability, RequestState, ReviewState};
use crate::write::{OpId, WriteState};
use ratatui::layout::Rect;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
    Info,
}
impl CheckStatus {
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Ok => "✓",
            Self::Warn => "⚠",
            Self::Fail => "✗",
            Self::Info => "·",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MergeCheck {
    pub status: CheckStatus,
    pub label: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MergeStage {
    Preflight,
    Confirm,
    ConfirmUnsafe,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MergeSession {
    pub id: ChangeRequestId,
    pub strategies: Vec<MergeStrategy>,
    pub strategy: usize,
    pub checks: Vec<MergeCheck>,
    pub warnings: Vec<String>,
    pub stage: MergeStage,
    pub write: WriteState<MergeOutcome>,
    pub submit_op: Option<OpId>,
    pub button_selected: usize,
    pub strategy_hits: Vec<Rect>,
    pub button_hits: Vec<(Rect, usize)>,
}

impl MergeSession {
    /// None when merging is unavailable or no strategy is supported: the UI simply never
    /// renders a merge action for that request.
    pub fn build(request: &ChangeRequest, caps: &ForgeCapabilities) -> Option<Self> {
        if !caps.merge || request.state != RequestState::Open {
            return None;
        }
        let candidates = [
            (MergeStrategy::Squash, caps.squash_merge),
            (MergeStrategy::MergeCommit, caps.merge_commit),
            (MergeStrategy::Rebase, caps.rebase_merge),
        ];
        let strategies: Vec<MergeStrategy> = candidates
            .into_iter()
            .filter_map(|(strategy, supported)| supported.then_some(strategy))
            .collect();
        if strategies.is_empty() {
            return None;
        }
        let mut checks = vec![];
        let mut warnings = vec![];
        let mut record = |status: CheckStatus, label: String| {
            if matches!(status, CheckStatus::Warn | CheckStatus::Fail) {
                warnings.push(label.clone());
            }
            checks.push(MergeCheck { status, label });
        };
        match request.ci {
            CiState::Passed => record(CheckStatus::Ok, "CI passed".into()),
            CiState::Failed => record(CheckStatus::Fail, "CI is failing".into()),
            CiState::Running => record(CheckStatus::Warn, "CI still running".into()),
            CiState::Pending => record(CheckStatus::Warn, "CI queued".into()),
            CiState::None => record(CheckStatus::Info, "no CI reported".into()),
        }
        let approvals = request
            .reviewers
            .iter()
            .filter(|reviewer| reviewer.state == ReviewState::Approved)
            .count();
        if approvals > 0 {
            record(CheckStatus::Ok, format!("{approvals} approval(s)"));
        } else {
            record(CheckStatus::Warn, "no approvals yet".into());
        }
        let blockers: Vec<String> = request
            .reviewers
            .iter()
            .filter(|reviewer| reviewer.state == ReviewState::ChangesRequested)
            .map(|reviewer| reviewer.person.login.clone())
            .collect();
        if blockers.is_empty() {
            record(CheckStatus::Ok, "no requested changes".into());
        } else {
            record(
                CheckStatus::Warn,
                format!("{} requested changes", blockers.join(", ")),
            );
        }
        match request.mergeability {
            Mergeability::Mergeable => record(CheckStatus::Ok, "no merge conflicts".into()),
            Mergeability::Conflicting => record(CheckStatus::Fail, "merge conflicts".into()),
            Mergeability::Blocked => record(
                CheckStatus::Warn,
                "provider reports a policy block".into(),
            ),
            Mergeability::Unknown => record(CheckStatus::Info, "mergeability unknown".into()),
        }
        match request.mergeable_state.as_deref() {
            Some("behind") => record(
                CheckStatus::Warn,
                "branch is behind the target branch".into(),
            ),
            Some("blocked") | Some("blocked_behind_merge") => record(
                CheckStatus::Warn,
                "branch protection blocks this merge".into(),
            ),
            Some("clean") => {}
            Some("draft") => {}
            Some("unstable") => record(
                CheckStatus::Warn,
                "required checks are not passing".into(),
            ),
            Some("dirty") => {}
            _ => record(CheckStatus::Info, "branch currency unknown".into()),
        }
        if request.draft {
            record(CheckStatus::Warn, "still a draft".into());
        }
        if request.auto_merge {
            record(
                CheckStatus::Info,
                "auto-merge enabled: merges when requirements pass".into(),
            );
        }
        if let Some(queue) = &request.merge_queue {
            let position = queue
                .position
                .map(|position| format!("position {position}"))
                .unwrap_or_else(|| "queued".into());
            record(CheckStatus::Info, format!("merge queue: {position}"));
        }
        Some(Self {
            id: request.id.clone(),
            strategies,
            strategy: 0,
            checks,
            warnings,
            stage: MergeStage::Preflight,
            write: WriteState::Idle,
            submit_op: None,
            button_selected: 0,
            strategy_hits: vec![],
            button_hits: vec![],
        })
    }
    pub fn selected_strategy(&self) -> MergeStrategy {
        self.strategies[self.strategy.min(self.strategies.len() - 1)]
    }
    /// Returns true when the session should close.
    pub fn handle_key(&mut self, app: &mut App, key: crossterm::event::KeyCode, modifiers: crossterm::event::KeyModifiers) -> bool {
        use crossterm::event::{KeyCode::*, KeyModifiers};
        if self.write.is_pending() {
            return false;
        }
        match self.stage {
            MergeStage::Preflight => match key {
                Esc => return true,
                Up | Char('k') => self.strategy = self.strategy.saturating_sub(1),
                Down | Char('j') => {
                    self.strategy = (self.strategy + 1).min(self.strategies.len() - 1)
                }
                Left | Char('h') => self.button_selected = self.button_selected.saturating_sub(1),
                Right | Char('l') => self.button_selected = self.button_selected.min(1),
                Enter if modifiers.contains(KeyModifiers::CONTROL) => self.begin_merge(app),
                Enter => {
                    if self.button_selected == 1 {
                        self.begin_merge(app);
                    }
                }
                _ => {}
            },
            MergeStage::Confirm | MergeStage::ConfirmUnsafe => match key {
                Esc => self.stage = MergeStage::Preflight,
                Left | Char('h') => self.button_selected = self.button_selected.saturating_sub(1),
                Right | Char('l') => self.button_selected = self.button_selected.min(1),
                Enter if modifiers.contains(KeyModifiers::CONTROL) => {
                    if self.button_selected == 1 {
                        self.submit(app);
                    } else {
                        self.stage = MergeStage::Preflight;
                    }
                }
                Enter => {
                    if self.button_selected == 1 {
                        self.submit(app);
                    } else {
                        self.stage = MergeStage::Preflight;
                    }
                }
                _ => {}
            },
        }
        false
    }
    pub fn begin_merge(&mut self, app: &mut App) {
        if self.warnings.is_empty() {
            self.stage = MergeStage::Confirm;
        } else {
            self.stage = MergeStage::ConfirmUnsafe;
        }
        self.button_selected = 0;
    }
    fn submit(&mut self, app: &mut App) {
        let op = OpId::next();
        self.write = WriteState::Pending;
        self.submit_op = Some(op);
        let strategy = self.selected_strategy();
        app.start_merge(self.id.clone(), strategy, op);
    }
    pub fn apply(&mut self, op: OpId, result: Result<MergeOutcome, crate::forge::ForgeError>) {
        if self.submit_op != Some(op) {
            return;
        }
        self.write = match result {
            Ok(outcome) => WriteState::Success(outcome),
            Err(error) => WriteState::Failed(error.to_string()),
        };
        self.stage = MergeStage::Preflight;
        self.submit_op = None;
    }
    pub fn failure(&self) -> Option<String> {
        match &self.write {
            WriteState::Failed(error) => Some(error.clone()),
            _ => None,
        }
    }
    pub fn handle_mouse(&mut self, app: &mut App, column: u16, row: u16) {
        let position = ratatui::layout::Position { x: column, y: row };
        for (rect, index) in self.strategy_hits.clone() {
            if rect.contains(position) {
                self.strategy = index.min(self.strategies.len() - 1);
                return;
            }
        }
        for (rect, index) in self.button_hits.clone() {
            if rect.contains(position) {
                self.button_selected = index;
                if self.stage == MergeStage::Preflight {
                    if index == 1 {
                        self.begin_merge(app);
                    }
                } else if index == 1 {
                    self.submit(app);
                } else {
                    self.stage = MergeStage::Preflight;
                }
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::create::full_caps;

    fn request() -> ChangeRequest {
        let mut request = crate::forge::demo::change_requests().remove(0);
        request.id.number = 184;
        request.draft = false;
        request.mergeability = Mergeability::Mergeable;
        request.ci = CiState::Passed;
        request.mergeable_state = Some("clean".into());
        request
    }

    #[test]
    fn preflight_collects_clean_checks_for_a_mergeable_request() {
        let session = MergeSession::build(&request(), &full_caps()).unwrap();
        assert!(session.warnings.is_empty());
        assert!(session
            .checks
            .iter()
            .any(|check| check.label == "CI passed" && check.status == CheckStatus::Ok));
    }

    #[test]
    fn preflight_warns_on_failing_ci_and_requested_changes() {
        let mut item = request();
        item.ci = CiState::Failed;
        item.reviewers = vec![crate::model::Reviewer {
            person: crate::model::Person::named("bob"),
            state: ReviewState::ChangesRequested,
        }];
        let session = MergeSession::build(&item, &full_caps()).unwrap();
        assert!(session
            .warnings
            .iter()
            .any(|warning| warning.contains("CI is failing")));
        assert!(session
            .warnings
            .iter()
            .any(|warning| warning.contains("bob requested changes")));
    }

    #[test]
    fn strategies_are_capability_gated() {
        let session = MergeSession::build(&request(), &full_caps()).unwrap();
        assert_eq!(
            session.strategies,
            vec![MergeStrategy::Squash, MergeStrategy::MergeCommit, MergeStrategy::Rebase]
        );
        let mut caps = full_caps();
        caps.squash_merge = false;
        caps.rebase_merge = false;
        let session = MergeSession::build(&request(), &caps).unwrap();
        assert_eq!(session.strategies, vec![MergeStrategy::MergeCommit]);
        caps.merge = false;
        assert!(MergeSession::build(&request(), &caps).is_none());
    }

    #[test]
    fn merge_is_unavailable_for_closed_or_merged_requests() {
        let mut item = request();
        item.state = RequestState::Merged;
        assert!(MergeSession::build(&item, &full_caps()).is_none());
    }

    #[test]
    fn warnings_require_an_explicit_merge_anyway_confirmation() {
        let mut app = App::test_app();
        let mut item = request();
        item.ci = CiState::Running;
        let mut session = MergeSession::build(&item, &full_caps()).unwrap();
        session.begin_merge(&app);
        assert_eq!(session.stage, MergeStage::ConfirmUnsafe);
        // Default button is Cancel.
        assert_eq!(session.button_selected, 0);
        session.handle_key(&app, crate::crossterm::event::KeyCode::Enter, KeyModifiers::NONE);
        assert!(!session.write.is_pending());

        // Confirming the unsafe path arms the write.
        session.button_selected = 1;
        session.submit(&mut app);
        assert!(session.write.is_pending());
        // A second submit is blocked.
        let op = session.submit_op;
        session.submit(&mut app);
        assert_eq!(session.submit_op, op);
    }

    #[test]
    fn clean_merge_goes_straight_to_confirmation_with_cancel_default() {
        let mut app = App::test_app();
        let mut session = MergeSession::build(&request(), &full_caps()).unwrap();
        session.begin_merge(&app);
        assert_eq!(session.stage, MergeStage::Confirm);
        assert_eq!(session.button_selected, 0);
        session.handle_key(&app, crate::crossterm::event::KeyCode::Enter, KeyModifiers::NONE);
        assert!(!session.write.is_pending());
    }

    #[test]
    fn stale_merge_results_are_ignored() {
        let mut session = MergeSession::build(&request(), &full_caps()).unwrap();
        session.apply(
            OpId(99),
            Ok(MergeOutcome {
                sha: Some("4e2f73a".into()),
                message: None,
            }),
        );
        assert!(session.write.is_idle());
    }
}
