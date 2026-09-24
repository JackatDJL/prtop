pub mod theme;
use crate::app::{
    App, BranchCleanupChoice, DetailFocus, HitRegions, Overlay, PALETTE_COMMANDS, View,
};
use crate::model::LoadState;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Wrap},
};

use theme::Theme;
pub fn draw(frame: &mut Frame, app: &mut App) {
    let theme = Theme::detect();
    if matches!(app.view, View::PipelineDetail(_)) {
        draw_pipeline(frame, app, theme);
        return;
    }
    if matches!(app.view, View::JobDetail(_)) {
        draw_job(frame, app, theme);
        return;
    }
    if matches!(app.view, View::ChangeRequestDetail(_)) {
        draw_full_detail(frame, app, theme);
        if app.show_help {
            help(frame);
        }
        if let Some(message) = app.toast() {
            toast(frame, message, theme);
        }
        draw_overlay(frame, app, theme);
        return;
    }
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(1),
        ])
        .split(frame.area());
    let refresh = app
        .last_refresh
        .map(|t| format!("updated {}s ago", t.elapsed().as_secs()))
        .unwrap_or_else(|| "loading".into());
    let title = Line::from(vec![
        Span::styled(
            " prtop ",
            Style::default()
                .fg(theme.background)
                .bg(theme.primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" All  GitHub  GitLab  Codeberg"),
        Span::raw(format!("                                      ↻ {refresh}")),
    ]);
    frame.render_widget(
        Paragraph::new(title).block(Block::default().borders(Borders::BOTTOM)),
        outer[0],
    );
    let panes = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(48), Constraint::Percentage(52)])
        .split(outer[1]);
    let detail_columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(panes[1]);
    app.set_regions(HitRegions {
        requests: panes[0],
        details: panes[1],
        description: detail_columns[0],
        comments: detail_columns[0],
        ci: detail_columns[1],
        reviewers: detail_columns[1],
        metadata: detail_columns[1],
        ..HitRegions::default()
    });
    draw_list(frame, panes[0], app, theme);
    draw_detail(frame, panes[1], app, theme);
    let health = app
        .health
        .iter()
        .map(|(name, status)| format!("{name} {status}"))
        .collect::<Vec<_>>()
        .join("  |  ");
    frame.render_widget(
        Paragraph::new(format!(
            " n create  : commands  / filter  r refresh  Enter detail  ? keys  q quit   {health}"
        ))
        .style(Style::default().fg(theme.muted)),
        outer[2],
    );
    if app.show_help {
        help(frame);
    }
    if let Some(message) = app.toast() {
        toast(frame, message, theme);
    }
    draw_overlay(frame, app, theme);
}
fn draw_full_detail(frame: &mut Frame, app: &mut App, theme: Theme) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(10),
            Constraint::Length(1),
        ])
        .split(frame.area());
    let Some(pr) = app.detail_request() else {
        frame.render_widget(
            Paragraph::new(
                "Change request is no longer available.\n\nEsc returns to the dashboard.",
            )
            .block(Block::default().borders(Borders::ALL).title(" detail ")),
            outer[1],
        );
        return;
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                format!(" {} {}", pr.id.display(pr.kind), pr.title),
                Style::default()
                    .fg(theme.foreground)
                    .add_modifier(Modifier::BOLD),
            ),
            Line::from(format!(
                " {} · {}   {} → {}",
                pr.id.forge, pr.id.repository, pr.source_branch, pr.target_branch
            )),
        ])
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border_active))
                .title(" change request "),
        ),
        outer[0],
    );
    let detail = outer[1];
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(detail);
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(6), Constraint::Min(8)])
        .split(columns[0]);
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(45),
            Constraint::Percentage(30),
            Constraint::Percentage(25),
        ])
        .split(columns[1]);
    app.set_regions(HitRegions {
        details: detail,
        description: left[0],
        comments: left[1],
        ci: right[0],
        reviewers: right[1],
        metadata: right[2],
        ..HitRegions::default()
    });
    draw_detail(frame, detail, app, theme);
    frame.render_widget(
        Paragraph::new(" ↑↓ scroll  PgUp/PgDn fast  Tab panel  e edit  M merge  : commands  c comment  R review  Esc back")
            .style(Style::default().fg(theme.muted)),
        outer[2],
    );
}
fn toast(frame: &mut Frame, message: &str, theme: Theme) {
    if message.trim().is_empty() || frame.area().width < 6 || frame.area().height < 3 {
        return;
    }
    let max_width = frame.area().width.saturating_sub(4);
    let width = (Span::raw(message).width() as u16)
        .saturating_add(6)
        .min(max_width);
    if width == 0 {
        return;
    }
    let area = Rect::new(
        frame
            .area()
            .x
            .saturating_add(2)
            .min(frame.area().right().saturating_sub(width)),
        frame.area().bottom().saturating_sub(3),
        width,
        2,
    );
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(format!(" ✓ {message} "))
            .style(Style::default().fg(theme.background).bg(theme.success))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.success)),
            ),
        area,
    );
}
fn draw_overlay(frame: &mut Frame, app: &mut App, theme: Theme) {
    let commands = app.palette_commands();
    let request = app.request_for_view().cloned();
    let comment_pending = app.comment_submission_pending();
    app.palette_hits.clear();
    let hits = &mut app.palette_hits;
    if let Some(overlay) = app.overlay.as_mut() {
        overlay_view(
            frame,
            overlay,
            &commands,
            request.as_ref(),
            hits,
            comment_pending,
            theme,
        );
    }
}

fn overlay_view(
    frame: &mut Frame,
    overlay: &mut Overlay,
    commands: &[&str],
    request: Option<&crate::model::ChangeRequest>,
    palette_hits: &mut Vec<(Rect, usize)>,
    comment_pending: bool,
    theme: Theme,
) {
    match overlay {
        Overlay::Composer {
            body,
            error,
            button_hits,
        } => draw_comment_composer(
            frame,
            body,
            error.as_deref(),
            button_hits,
            comment_pending,
            theme,
        ),
        Overlay::Create(session) => draw_create(frame, session, theme),
        Overlay::Edit(session) => draw_edit(frame, session, request, theme),
        Overlay::Merge(session) => draw_merge(frame, session, theme),
        Overlay::Confirm(dialog) => draw_confirm(frame, dialog, theme),
        Overlay::BranchCleanup(session) => draw_cleanup(frame, session, theme),
        Overlay::Palette { query, selected } => {
            let area = centered(frame.area(), 64, 60);
            frame.render_widget(Clear, area);
            let matching: Vec<_> = commands
                .iter()
                .filter(|item| item.to_lowercase().contains(&query.to_lowercase()))
                .collect();
            let mut lines = vec![Line::styled(
                format!("{query}_"),
                Style::default().fg(theme.primary),
            )];
            lines.extend(matching.iter().enumerate().map(|(index, command)| {
                Line::from(format!(
                    "{} {command}",
                    if index == *selected { ">" } else { " " }
                ))
            }));
            frame.render_widget(
                Paragraph::new(lines).wrap(Wrap { trim: false }).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Command palette "),
                ),
                area,
            );
            palette_hits.extend((0..matching.len()).map(|index| {
                (
                    Rect::new(
                        area.x + 1,
                        area.y + index as u16 + 2,
                        area.width.saturating_sub(2),
                        1,
                    ),
                    index,
                )
            }));
        }
        _ => overlay_legacy(frame, overlay, theme),
    }
}

fn draw_comment_composer(
    frame: &mut Frame,
    body: &str,
    error: Option<&str>,
    button_hits: &mut Vec<(Rect, usize)>,
    pending: bool,
    theme: Theme,
) {
    let area = centered(frame.area(), 72, 58);
    frame.render_widget(Clear, area);
    let mut lines = if body.is_empty() {
        vec![Line::styled(
            "Write a comment…",
            Style::default().fg(theme.muted),
        )]
    } else {
        body.lines()
            .map(|line| Line::from(line.to_owned()))
            .collect()
    };
    if pending {
        lines.push(Line::styled(
            "Posting comment…",
            Style::default().fg(theme.primary),
        ));
    }
    if let Some(error) = error {
        lines.push(Line::styled(
            format!("Failed to post comment: {error}"),
            Style::default().fg(theme.danger),
        ));
        lines.push(Line::from("Ctrl+Enter retries · Esc keeps the draft"));
    }
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border_active))
                .title(" Add comment "),
        ),
        area,
    );
    let submit_label = if pending {
        "Posting…"
    } else if error.is_some() {
        "Retry"
    } else {
        "Post comment"
    };
    button_hits.clear();
    button_hits.extend(
        button_rects(frame, area, &[("Cancel", 0), (submit_label, 1)], 1, theme)
            .into_iter()
            .enumerate()
            .map(|(index, rect)| (rect, index)),
    );
}

fn draw_create(frame: &mut Frame, session: &mut crate::create::CreateWorkflow, theme: Theme) {
    let area = centered(frame.area(), 88, 82);
    frame.render_widget(Clear, area);
    session.field_hits.clear();
    session.button_hits.clear();
    session.mouse_area = None;
    let body = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(4));
    let mut lines = Vec::new();
    match session.stage {
        crate::create::CreateStage::Preflight => {
            lines.extend(session.summary_lines().into_iter().map(Line::from));
            if session.preflight_loading {
                lines.push(Line::from("Git preflight running…"));
            }
            if let Some(error) = &session.preflight_error {
                lines.push(Line::styled(
                    format!("Preflight failed: {error}"),
                    Style::default().fg(theme.danger),
                ));
            }
            if session.push_needed() {
                let branch = session
                    .preflight
                    .as_ref()
                    .map(|state| state.branch.as_str())
                    .unwrap_or("unknown");
                if session.remote_branch_exists == Some(false) {
                    lines.push(Line::from(format!(
                        "Branch {branch} does not exist on the remote."
                    )));
                } else if let Some(count) =
                    session.preflight.as_ref().and_then(|state| state.unpushed)
                {
                    lines.push(Line::from(format!(
                        "{count} source commit(s) are not pushed."
                    )));
                }
                lines.push(Line::from(
                    "Push & Continue runs git push -u; force push is never used.",
                ));
            }
            if session.preflight_error.is_some() {
                lines.push(Line::from("Press r to retry Git preflight."));
            }
            if let Some(error) = &session.push_error {
                lines.push(Line::styled(
                    format!("Push failed: {error}"),
                    Style::default().fg(theme.danger),
                ));
            }
        }
        crate::create::CreateStage::Fields => {
            lines.push(Line::from("Source branch: current checkout"));
            for (index, field) in session.fields().iter().enumerate() {
                lines.push(Line::from(format!(
                    "{} {:<14} {}",
                    if index == session.field { ">" } else { " " },
                    field.label(),
                    session.field_summary(*field)
                )));
                session.field_hits.push((
                    Rect::new(
                        body.x + 1,
                        body.y + index as u16 + 2,
                        body.width.saturating_sub(2),
                        1,
                    ),
                    index,
                ));
            }
            if let Some(editor) = &session.editor {
                lines.push(Line::from(""));
                match editor {
                    crate::create::CreateEditor::Title(area) => {
                        lines.push(Line::styled(
                            format!("Title: {}_", area.text()),
                            Style::default().fg(theme.primary),
                        ));
                        lines.push(Line::from(
                            "Enter moves to the next field · Ctrl+Enter previews",
                        ));
                    }
                    crate::create::CreateEditor::Description => {
                        lines.extend(
                            session
                                .body
                                .text()
                                .lines()
                                .map(|line| Line::from(line.to_owned())),
                        );
                        lines.push(Line::from(
                            "Description · Ctrl+Enter previews · Esc keeps text",
                        ));
                    }
                    crate::create::CreateEditor::Target(picker)
                    | crate::create::CreateEditor::Reviewers(picker)
                    | crate::create::CreateEditor::Labels(picker)
                    | crate::create::CreateEditor::Assignees(picker)
                    | crate::create::CreateEditor::Milestone(picker) => {
                        lines.push(Line::styled(
                            format!("{}: {}_", picker.kind.title(), picker.query),
                            Style::default().fg(theme.primary),
                        ));
                        lines.push(Line::from(picker.status_line()));
                        let top = body.y + lines.len() as u16 + 1;
                        for (index, item) in picker.filtered().iter().enumerate().take(12) {
                            lines.push(Line::from(format!(
                                "{} {}{}",
                                if index == picker.selected { ">" } else { " " },
                                if picker.is_checked(&item.id) {
                                    "[x] "
                                } else {
                                    "[ ] "
                                },
                                item.label
                            )));
                        }
                        if picker.multi {
                            lines.push(Line::from("Space toggles · Ctrl+Enter applies"));
                        }
                        session.mouse_area = Some(Rect::new(
                            body.x + 1,
                            top,
                            body.width.saturating_sub(2),
                            picker.visible_count().min(12) as u16,
                        ));
                    }
                }
            }
        }
        crate::create::CreateStage::Preview => {
            lines.extend([
                Line::styled(
                    &session.title,
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Line::from(format!(
                    "{} → {}",
                    session
                        .preflight
                        .as_ref()
                        .map(|state| state.branch.as_str())
                        .unwrap_or("?"),
                    session.target
                )),
                Line::from(""),
            ]);
            lines.extend(
                session
                    .body
                    .text()
                    .lines()
                    .map(|line| Line::from(line.to_owned())),
            );
            for field in [
                crate::create::Field::Draft,
                crate::create::Field::Reviewers,
                crate::create::Field::Labels,
                crate::create::Field::Assignees,
                crate::create::Field::Milestone,
            ] {
                lines.push(Line::from(format!(
                    "{}: {}",
                    field.label(),
                    session.field_summary(field)
                )));
            }
            if let Some(error) = session.failure_message() {
                lines.push(Line::styled(
                    format!("Create failed: {error}"),
                    Style::default().fg(theme.danger),
                ));
            }
            if session.submit.is_pending() {
                lines.push(Line::from("Creating request…"));
            }
        }
    }
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::default().borders(Borders::ALL).title(format!(
                " Create {} · {}/{} ",
                session.kind.number_prefix(),
                session.host,
                session.repository
            )),
        ),
        body,
    );
    let buttons = if session.stage == crate::create::CreateStage::Fields
        && session.editor.as_ref().is_some_and(|editor| {
            matches!(
                editor,
                crate::create::CreateEditor::Target(_)
                    | crate::create::CreateEditor::Reviewers(_)
                    | crate::create::CreateEditor::Labels(_)
                    | crate::create::CreateEditor::Assignees(_)
                    | crate::create::CreateEditor::Milestone(_)
            )
        }) {
        vec![
            (crate::create::Button::Cancel, "Cancel picker"),
            (crate::create::Button::Continue, "Apply selection"),
        ]
    } else {
        match session.stage {
            crate::create::CreateStage::Preflight if session.push_needed() => vec![
                (crate::create::Button::Cancel, "Cancel"),
                (crate::create::Button::PushAndContinue, "Push & Continue"),
            ],
            crate::create::CreateStage::Preflight => vec![
                (crate::create::Button::Cancel, "Cancel"),
                (crate::create::Button::Continue, "Continue"),
            ],
            crate::create::CreateStage::Fields => vec![
                (crate::create::Button::Cancel, "Cancel"),
                (crate::create::Button::Continue, "Preview"),
            ],
            crate::create::CreateStage::Preview => vec![
                (crate::create::Button::Cancel, "Back to fields"),
                (crate::create::Button::Create, "Create"),
            ],
        }
    };
    let raw = buttons
        .iter()
        .map(|(button, label)| (*label, *button as usize))
        .collect::<Vec<_>>();
    session.button_hits = button_rects(frame, area, &raw, session.button_selected, theme)
        .into_iter()
        .enumerate()
        .map(|(index, rect)| (rect, buttons[index].0))
        .collect();
}

fn draw_edit(
    frame: &mut Frame,
    session: &mut crate::edit::EditSession,
    request: Option<&crate::model::ChangeRequest>,
    theme: Theme,
) {
    let area = centered(frame.area(), 76, 72);
    frame.render_widget(Clear, area);
    session.item_hits.clear();
    session.mouse_area = None;
    session.button_hits.clear();
    let mut picker_buttons = false;
    let mut lines = Vec::new();
    if let Some(crate::edit::EditField::Title(editor)) = &session.active {
        lines.extend([
            Line::styled(
                format!("Title: {}_", editor.text()),
                Style::default().fg(theme.primary),
            ),
            Line::from("Ctrl+Enter submits · Esc cancels"),
        ]);
    } else if let Some(crate::edit::EditField::Body(editor)) = &session.active {
        lines.extend(
            editor
                .text()
                .lines()
                .map(|line| Line::from(line.to_owned())),
        );
        lines.push(Line::from("Description · Ctrl+Enter submits · Esc cancels"));
    } else if let Some(crate::edit::EditField::Picker(picker)) = &session.active {
        lines.extend([
            Line::styled(
                format!("{}: {}_", picker.kind.title(), picker.query),
                Style::default().fg(theme.primary),
            ),
            Line::from(picker.status_line()),
        ]);
        if picker.multi {
            lines.push(Line::from("Space toggles · Apply selection below"));
        }
        let top = area.y + if picker.multi { 4 } else { 3 };
        for (index, item) in picker.filtered().iter().enumerate().take(14) {
            lines.push(Line::from(format!(
                "{} {}{}",
                if index == picker.selected { ">" } else { " " },
                if picker.is_checked(&item.id) {
                    "[x] "
                } else {
                    "[ ] "
                },
                item.label
            )));
        }
        session.mouse_area = Some(Rect::new(
            area.x + 2,
            top,
            area.width.saturating_sub(4),
            picker.visible_count().min(14) as u16,
        ));
        if picker.multi || picker.kind == crate::picker::PickerKind::Milestone {
            picker_buttons = true;
        }
    } else if let Some(request) = request {
        for (index, item) in session.menu_items(request).iter().enumerate() {
            lines.push(Line::from(format!(
                "{} {}",
                if index == session.menu_selected {
                    ">"
                } else {
                    " "
                },
                item.name
            )));
            session.item_hits.push((
                Rect::new(
                    area.x + 2,
                    area.y + index as u16 + 1,
                    area.width.saturating_sub(4),
                    1,
                ),
                index,
            ));
        }
        lines.push(Line::from("↑↓ select · Enter open · Esc close"));
    } else {
        lines.push(Line::from("Request is no longer available"));
    }
    if let Some(label) = session.pending_label() {
        lines.push(Line::from(format!("{label}…")));
    }
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Edit change request "),
        ),
        area,
    );
    if picker_buttons {
        session.button_hits = button_rects(
            frame,
            area,
            &[("Cancel", 0), ("Apply selection", 1)],
            0,
            theme,
        )
        .into_iter()
        .enumerate()
        .map(|(index, rect)| (rect, index))
        .collect();
    }
}

fn draw_merge(frame: &mut Frame, session: &mut crate::merge::MergeSession, theme: Theme) {
    let area = centered(frame.area(), 78, 78);
    frame.render_widget(Clear, area);
    session.strategy_hits.clear();
    let technical = match session.technical_mergeability {
        crate::model::Mergeability::Mergeable if session.technically_mergeable => "mergeable",
        crate::model::Mergeability::Conflicting => "conflicts block merge",
        _ => "not confirmed by provider",
    };
    let mut lines = vec![
        Line::styled(
            format!("Technical mergeability: {technical}"),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Line::from(format!(
            "Policy requirements from provider status: {}",
            if session.policy_satisfied {
                "satisfied"
            } else {
                "warnings remain"
            }
        )),
        Line::from(""),
    ];
    if session.loading {
        lines.push(Line::from("Refreshing provider merge state…"));
    } else if let Some(error) = &session.preflight_error {
        lines.push(Line::styled(
            format!("Preflight failed: {error} · r retries"),
            Style::default().fg(theme.danger),
        ));
    } else {
        for check in &session.checks {
            lines.push(Line::from(format!(
                "{} {}",
                check.status.glyph(),
                check.label
            )));
        }
        lines.extend([
            Line::from(""),
            Line::styled(
                "Merge strategy",
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]);
        let strategy_top = area.y + lines.len() as u16 + 1;
        for (index, strategy) in session.strategies.iter().enumerate() {
            lines.push(Line::from(format!(
                "{} {}",
                if index == session.strategy {
                    "●"
                } else {
                    "○"
                },
                strategy.label()
            )));
            session.strategy_hits.push((
                Rect::new(
                    area.x + 2,
                    strategy_top + index as u16,
                    area.width.saturating_sub(4),
                    1,
                ),
                index,
            ));
        }
    }
    if let Some(error) = session.failure() {
        lines.push(Line::styled(
            format!("Merge failed: {error}"),
            Style::default().fg(theme.danger),
        ));
    }
    if session.write.is_pending() {
        lines.push(Line::from("Merge request pending…"));
    }
    match session.stage {
        crate::merge::MergeStage::Preflight => {}
        crate::merge::MergeStage::Confirm => lines.push(Line::styled(
            "Ready. Select Merge to submit.",
            Style::default().fg(theme.success),
        )),
        crate::merge::MergeStage::ConfirmUnsafe => lines.push(Line::styled(
            "Warnings remain. Confirm again to merge anyway.",
            Style::default().fg(theme.danger),
        )),
    }
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Merge preflight "),
        ),
        area,
    );
    let right = match session.stage {
        crate::merge::MergeStage::Preflight if session.loading => "Loading…",
        crate::merge::MergeStage::Preflight if session.preflight_error.is_some() => {
            "Retry preflight"
        }
        crate::merge::MergeStage::Preflight => {
            if session.technically_mergeable {
                "Review merge"
            } else {
                "Merge blocked"
            }
        }
        _ => "Merge",
    };
    session.button_hits = button_rects(
        frame,
        area,
        &[("Cancel", 0), (right, 1)],
        session.button_selected,
        theme,
    )
    .into_iter()
    .enumerate()
    .map(|(index, rect)| (rect, index))
    .collect();
}

fn draw_confirm(frame: &mut Frame, dialog: &mut crate::app::ConfirmDialog, theme: Theme) {
    let area = centered(frame.area(), 58, 38);
    frame.render_widget(Clear, area);
    let body = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(4));
    frame.render_widget(
        Paragraph::new(format!("{}\n\n{}", dialog.title, dialog.body))
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Confirm action "),
            ),
        body,
    );
    dialog.button_hits = button_rects(
        frame,
        area,
        &[(dialog.cancel.as_str(), 0), (dialog.confirm.as_str(), 1)],
        dialog.selected,
        theme,
    )
    .into_iter()
    .enumerate()
    .map(|(index, rect)| (rect, index))
    .collect();
}

fn draw_cleanup(frame: &mut Frame, session: &mut crate::app::BranchCleanupSession, _theme: Theme) {
    let area = centered(frame.area(), 68, 48);
    frame.render_widget(Clear, area);
    let mut lines = vec![
        Line::styled(
            format!("Merged {}", session.id.display(session.kind)),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Line::from(format!("Source branch: {}", session.branch)),
        Line::from(""),
    ];
    for (index, choice) in session.choices.iter().enumerate() {
        let label = match choice {
            BranchCleanupChoice::KeepBranches => "Keep branches",
            BranchCleanupChoice::DeleteRemote => "Delete remote source branch",
            BranchCleanupChoice::DeleteLocal => "Delete local source branch",
            BranchCleanupChoice::DeleteBoth => "Delete local and remote branches",
        };
        lines.push(Line::from(format!(
            "{} {label}",
            if index == session.selected { ">" } else { " " }
        )));
    }
    if session.pending {
        lines.push(Line::from("Deleting source branch…"));
    }
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Branch cleanup "),
        ),
        area,
    );
    session.button_hits = session
        .choices
        .iter()
        .enumerate()
        .map(|(index, _)| {
            (
                Rect::new(
                    area.x + 2,
                    area.y + 4 + index as u16,
                    area.width.saturating_sub(4),
                    1,
                ),
                index,
            )
        })
        .collect();
}

fn button_rects<T: Copy>(
    frame: &mut Frame,
    area: Rect,
    buttons: &[(&str, T)],
    selected: usize,
    theme: Theme,
) -> Vec<Rect> {
    let y = area.bottom().saturating_sub(3);
    let mut x = area.x + 2;
    let mut hits = Vec::new();
    for (index, (label, _)) in buttons.iter().enumerate() {
        let width = (label.len() as u16 + 4).min(frame.area().right().saturating_sub(x));
        let rect = Rect::new(x, y, width, 1);
        let style = if index == selected {
            Style::default()
                .fg(theme.selection_fg)
                .bg(theme.selection_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        frame.render_widget(Paragraph::new(format!("[ {label} ]")).style(style), rect);
        hits.push(rect);
        x = x.saturating_add(width + 2);
    }
    hits
}

fn overlay_legacy(frame: &mut Frame, overlay: &Overlay, theme: Theme) {
    let area = centered(frame.area(), 62, 42);
    frame.render_widget(Clear, area);
    let (title, body) = match overlay {
        Overlay::Composer { body, error, .. } => (
            "Add comment",
            format!(
                "{}\n\n{}",
                body,
                error
                    .as_ref()
                    .map(|error| format!("Failed to post comment: {error}\nCtrl+Enter retries"))
                    .unwrap_or_else(|| "Enter adds a line · Ctrl+Enter posts · Esc cancels".into())
            ),
        ),
        Overlay::ReviewMenu { selected } => {
            let options = ["Approve", "Request changes", "Comment"];
            (
                "Review",
                options
                    .iter()
                    .enumerate()
                    .map(|(index, item)| {
                        format!("{} {item}", if index == *selected { ">" } else { " " })
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        }
        Overlay::Palette { query, selected } => (
            "Command palette",
            format!(
                "{}\n\n{}",
                query,
                PALETTE_COMMANDS
                    .iter()
                    .filter(|command| command.to_lowercase().contains(&query.to_lowercase()))
                    .enumerate()
                    .map(|(index, command)| format!(
                        "{} {command}",
                        if index == *selected { ">" } else { " " }
                    ))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        ),
        Overlay::ConfirmDelete => (
            "Delete comment",
            "Delete this comment?\n\nEnter / Esc cancel    d delete".into(),
        ),
        Overlay::ConfirmCi { action } => (
            "Confirm CI action",
            format!(
                "{}?\n\nEnter / Esc cancels    y confirms",
                match action {
                    crate::app::CiAction::RetryJob(_) => "Retry job",
                    crate::app::CiAction::RetryPipeline(_) => "Retry pipeline",
                    crate::app::CiAction::CancelJob(_) => "Cancel running job",
                    crate::app::CiAction::CancelPipeline(_) => "Cancel running pipeline",
                    crate::app::CiAction::PlayJob(_) => "Start manual job",
                }
            ),
        ),
        _ => ("", String::new()),
    };
    frame.render_widget(
        Paragraph::new(body).wrap(Wrap { trim: false }).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border_active))
                .title(title),
        ),
        area,
    );
}
fn draw_list(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    let visible = app.visible();
    if visible.is_empty() && app.filter.is_empty() && app.repo_context.repository.is_some() {
        let mut lines = vec![
            Line::styled(
                "No open pull requests or merge requests.",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Line::from(format!(
                "{} / {}",
                app.repo_context.host.as_deref().unwrap_or("forge"),
                app.repo_context.repository.as_deref().unwrap_or_default()
            )),
        ];
        if let Some(state) = &app.project_git {
            lines.push(Line::from(format!("Current branch: {}", state.branch)));
            lines.push(Line::from(format!(
                "{} commits ahead of {}",
                state
                    .ahead
                    .map_or_else(|| "?".into(), |count| count.to_string()),
                app.repo_context
                    .default_branch
                    .as_deref()
                    .unwrap_or("target")
            )));
        } else {
            lines.push(Line::from("Checking the current Git branch…"));
        }
        if app.can_create_current_project() {
            lines.push(Line::styled(
                "[n] Create pull request",
                Style::default().fg(theme.primary),
            ));
        }
        frame.render_widget(
            Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" project ")),
            area,
        );
        return;
    }
    let rows: Vec<Row<'_>> = visible
        .iter()
        .enumerate()
        .map(|(i, pr)| {
            Row::new([
                Cell::from(if i == app.selected { ">" } else { " " }),
                Cell::from(pr.id.forge.as_str()),
                Cell::from(pr.id.repository.as_str()),
                Cell::from(pr.id.display(pr.kind)),
                Cell::from(pr.title.as_str()),
                Cell::from(format!("{} {}", pr.ci.glyph(), pr.ci.label())),
                Cell::from(format!("{} {}", pr.review.glyph(), pr.review.label())),
            ])
            .style(if i == app.selected {
                Style::default()
                    .fg(theme.selection_fg)
                    .bg(theme.selection_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            })
        })
        .collect();
    let layout = request_table_layout(area.width);
    frame.render_widget(
        Table::new(rows, layout.widths)
            .header(
                Row::new([
                    Cell::from(""),
                    Cell::from("forge"),
                    Cell::from("repository"),
                    Cell::from("id"),
                    Cell::from("title"),
                    Cell::from("ci"),
                    Cell::from("review"),
                ])
                .style(
                    Style::default()
                        .fg(theme.secondary)
                        .add_modifier(Modifier::BOLD),
                ),
            )
            .column_spacing(layout.spacing)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.border_active))
                    .title(if app.filtering {
                        format!(" requests  filter: {}_ ", app.filter)
                    } else if app.filter.is_empty() {
                        " requests ".to_owned()
                    } else {
                        " requests  filter ".to_owned()
                    }),
            ),
        area,
    );
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RequestTableLayout {
    widths: [u16; 7],
    spacing: u16,
}

fn request_table_layout(area_width: u16) -> RequestTableLayout {
    let spacing: u16 = if area_width >= 28 { 1 } else { 0 };
    let available = area_width
        .saturating_sub(2)
        .saturating_sub(spacing.saturating_mul(6));
    let minimums = [1, 3, 4, 2, 0, 1, 1];
    if available < minimums.iter().copied().sum::<u16>() {
        let mut widths = [0; 7];
        let mut remaining = available;
        for index in [0, 1, 2, 3, 5, 6, 4] {
            widths[index] = remaining.min(minimums[index]);
            remaining -= widths[index];
        }
        return RequestTableLayout { widths, spacing };
    }
    let mut widths = [2, 10, 24, 6, 12, 8, 8];
    let mut excess = widths
        .iter()
        .copied()
        .sum::<u16>()
        .saturating_sub(available);
    // Keep useful metadata first on narrow terminals; title truncates before the optional
    // status labels compact down to their Unicode glyphs.
    for index in [5, 6, 4, 2, 1, 3, 0] {
        let shrink = excess.min(widths[index].saturating_sub(minimums[index]));
        widths[index] -= shrink;
        excess -= shrink;
    }
    if excess == 0 && widths.iter().copied().sum::<u16>() < available {
        let spare = available.saturating_sub(widths.iter().copied().sum::<u16>());
        let repository_growth = (spare / 3).min(42_u16.saturating_sub(widths[2]));
        widths[2] = widths[2].saturating_add(repository_growth);
        widths[4] = widths[4].saturating_add(spare.saturating_sub(repository_growth));
    }
    RequestTableLayout { widths, spacing }
}
fn draw_detail(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(6), Constraint::Min(8)])
        .split(columns[0]);
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(45),
            Constraint::Percentage(30),
            Constraint::Percentage(25),
        ])
        .split(columns[1]);
    let Some(pr) = app.request_for_view() else {
        frame.render_widget(
            Paragraph::new("No requests. Configure a forge or use --demo.")
                .block(Block::default().borders(Borders::ALL).title(" detail ")),
            area,
        );
        return;
    };
    let dashboard_preview = matches!(&app.view, View::Dashboard);
    let resources = app.detail_resources_for(&pr.id);
    let mut description = vec![
        Line::styled(&pr.title, Style::default().add_modifier(Modifier::BOLD)),
        Line::from(format!(
            "{} · {} → {} · {} {}",
            pr.id.display(pr.kind),
            pr.source_branch,
            pr.target_branch,
            pr.state.glyph(),
            pr.state.label()
        )),
        Line::from(format!(
            "+{} / -{} · {}",
            pr.additions,
            pr.deletions,
            pr.mergeability.label()
        )),
    ];
    match resources.map(|state| &state.request) {
        Some(LoadState::Loaded(())) => {}
        Some(LoadState::Failed(error)) => description.push(Line::styled(
            format!("Request details unavailable: {error} · press r to retry"),
            Style::default().fg(theme.danger),
        )),
        Some(LoadState::Unsupported) => description.push(Line::from("Request details unsupported")),
        Some(LoadState::NotLoaded) | None if dashboard_preview => {
            description.push(Line::from("Enter to load full request details"))
        }
        Some(LoadState::NotLoaded | LoadState::Loading) | None => {
            description.push(Line::from("Loading request details…"))
        }
    }
    frame.render_widget(
        Paragraph::new(description).block(panel_block(
            " description ",
            app.detail_focus == DetailFocus::Description,
            theme,
        )),
        left[0],
    );
    let mut comment_lines = match resources.map(|state| &state.comments) {
        Some(LoadState::Loaded(comments)) => {
            let visible = comments
                .iter()
                .rev()
                .skip(app.comment_scroll)
                .take(10)
                .map(|comment| {
                    Line::from(format!(
                        "{} · {}{}\n{}",
                        comment
                            .author
                            .name
                            .as_deref()
                            .unwrap_or(&comment.author.login),
                        comment.created_at.format("%H:%M"),
                        if comment.updated_at.is_some() {
                            " edited"
                        } else {
                            ""
                        },
                        comment.body.replace('\n', " ")
                    ))
                })
                .collect::<Vec<_>>();
            if comments.is_empty() {
                vec![
                    Line::styled("latest 0 of 0", Style::default().fg(theme.secondary)),
                    Line::from("No comments yet."),
                ]
            } else {
                let mut lines = vec![Line::styled(
                    format!("latest {} of {}", visible.len(), comments.len()),
                    Style::default().fg(theme.secondary),
                )];
                lines.extend(visible);
                lines
            }
        }
        Some(LoadState::Failed(error)) => vec![
            Line::styled("Comments unavailable", Style::default().fg(theme.danger)),
            Line::from(error.as_str()),
            Line::from("press r to retry"),
        ],
        Some(LoadState::Unsupported) => vec![Line::from("Comments unsupported by this forge")],
        Some(LoadState::NotLoaded) | None if dashboard_preview => {
            vec![Line::from("Enter to load comments")]
        }
        Some(LoadState::NotLoaded | LoadState::Loading) | None => {
            vec![Line::from("Loading comments…")]
        }
    };
    if let Some(activity) = app.activity.get(&pr.id) {
        comment_lines.extend(activity.iter().rev().map(|event| {
            Line::styled(
                format!("prtop activity · {event}"),
                Style::default().fg(theme.secondary),
            )
        }));
    }
    frame.render_widget(
        Paragraph::new(comment_lines)
            .wrap(Wrap { trim: true })
            .block(panel_block(
                " comments ",
                app.detail_focus == DetailFocus::Comments,
                theme,
            )),
        left[1],
    );
    let ci = match resources.map(|state| &state.ci) {
        Some(LoadState::Loaded(pipelines)) if pipelines.is_empty() => {
            vec![Line::from("No pipeline reported")]
        }
        Some(LoadState::Loaded(pipelines)) => pipelines
            .iter()
            .skip(app.ci_scroll)
            .take(8)
            .map(|pipeline| {
                Line::from(format!(
                    "{} {:<24} {}",
                    pipeline.status.glyph(),
                    pipeline.name,
                    pipeline.status.label()
                ))
            })
            .collect(),
        Some(LoadState::Failed(error)) => vec![
            Line::styled("CI unavailable", Style::default().fg(theme.danger)),
            Line::from(error.as_str()),
            Line::from("press r to retry"),
        ],
        Some(LoadState::Unsupported) => vec![Line::from("CI unsupported by this forge")],
        Some(LoadState::NotLoaded) | None if dashboard_preview => {
            vec![Line::from("Enter to load CI")]
        }
        Some(LoadState::NotLoaded | LoadState::Loading) | None => {
            vec![Line::from("Loading CI…")]
        }
    };
    frame.render_widget(
        Paragraph::new(ci).block(panel_block(
            " CI ",
            app.detail_focus == DetailFocus::Ci,
            theme,
        )),
        right[0],
    );
    let reviewers = match resources.map(|state| &state.reviews) {
        Some(LoadState::Loaded(reviewers)) if reviewers.is_empty() => {
            vec![Line::from("No reviewers reported")]
        }
        Some(LoadState::Loaded(reviewers)) => reviewers
            .iter()
            .map(|reviewer| {
                Line::from(format!(
                    "{} {:<16} {}",
                    reviewer.state.glyph(),
                    reviewer
                        .person
                        .name
                        .as_deref()
                        .unwrap_or(&reviewer.person.login),
                    reviewer.state.label()
                ))
            })
            .collect(),
        Some(LoadState::Failed(error)) => vec![
            Line::styled("Reviewers unavailable", Style::default().fg(theme.danger)),
            Line::from(error.as_str()),
            Line::from("press r to retry"),
        ],
        Some(LoadState::Unsupported) => vec![Line::from("Reviewer data unsupported by this forge")],
        Some(LoadState::NotLoaded) | None if dashboard_preview => {
            vec![Line::from("Enter to load reviewers")]
        }
        Some(LoadState::NotLoaded | LoadState::Loading) | None => {
            vec![Line::from("Loading reviewers…")]
        }
    };
    frame.render_widget(
        Paragraph::new(reviewers).block(panel_block(
            " reviewers ",
            app.detail_focus == DetailFocus::Reviewers,
            theme,
        )),
        right[1],
    );
    let metadata = vec![
        Line::from(format!("{} / {}", pr.id.forge, pr.id.repository)),
        Line::from(format!("author: {}", pr.author.login)),
        Line::from(format!("review: {}", pr.review.label())),
    ];
    frame.render_widget(
        Paragraph::new(metadata).block(panel_block(
            " metadata ",
            app.detail_focus == DetailFocus::Metadata,
            theme,
        )),
        right[2],
    );
}
fn draw_pipeline(frame: &mut Frame, app: &mut App, theme: Theme) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(6),
            Constraint::Length(1),
        ])
        .split(frame.area());
    let Some(pipeline) = app.pipeline_for_view() else {
        return;
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                format!(" {} · {}", pipeline.name, pipeline.ref_name),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Line::from(format!(" {} · {}", pipeline.sha, pipeline.status.label())),
        ])
        .block(Block::default().borders(Borders::ALL).title(" pipeline ")),
        outer[0],
    );
    let rows = pipeline
        .jobs
        .iter()
        .enumerate()
        .map(|(index, job)| {
            Line::from(format!(
                "{} {:<14} {:<28} {:<10} {}",
                if index == app.job_selected { ">" } else { " " },
                job.stage.as_deref().unwrap_or("-"),
                job.name,
                job.status.label(),
                job.duration_seconds
                    .map(|seconds| format!("{seconds}s"))
                    .unwrap_or_default()
            ))
        })
        .collect::<Vec<_>>();
    app.set_regions(HitRegions {
        jobs: outer[1],
        ..HitRegions::default()
    });
    frame.render_widget(
        Paragraph::new(rows).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" stage          job                          status      time "),
        ),
        outer[1],
    );
    frame.render_widget(
        Paragraph::new(" ↑↓ select  Enter logs  R retry pipeline  x cancel  r refresh  Esc back")
            .style(Style::default().fg(theme.muted)),
        outer[2],
    );
    if let Some(message) = app.toast() {
        toast(frame, message, theme);
    }
    draw_overlay(frame, app, theme);
}
fn draw_job(frame: &mut Frame, app: &mut App, theme: Theme) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .split(frame.area());
    let Some(job) = app.job_for_view() else {
        return;
    };
    let id = job.id.clone();
    let name = job.name.clone();
    let status = job.status;
    let lines = app.logs.get(&id).cloned().unwrap_or_default();
    let skip = if app.follow_logs {
        lines.len().saturating_sub(100)
    } else {
        lines
            .len()
            .saturating_sub(100)
            .saturating_sub(app.log_scroll)
    };
    let query = app.log_query.as_deref().unwrap_or("");
    let rendered = lines
        .iter()
        .skip(skip)
        .take(100)
        .map(|line| {
            Line::styled(
                line.clone(),
                if !query.is_empty() && line.to_lowercase().contains(&query.to_lowercase()) {
                    Style::default()
                        .fg(theme.warning)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                },
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                format!(" {name} · {}", status.label()),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Line::from(format!(
                " follow: {}  search: {}",
                if app.follow_logs { "on" } else { "off" },
                query
            )),
        ])
        .block(Block::default().borders(Borders::ALL).title(" job ")),
        outer[0],
    );
    app.set_regions(HitRegions {
        logs: outer[1],
        ..HitRegions::default()
    });
    frame.render_widget(
        Paragraph::new(rendered).block(Block::default().borders(Borders::ALL).title(" logs ")),
        outer[1],
    );
    frame.render_widget(
        Paragraph::new(" ↑↓ scroll  f follow  / search  n/N next/previous  R retry  Esc back")
            .style(Style::default().fg(theme.muted)),
        outer[2],
    );
    if let Some(message) = app.toast() {
        toast(frame, message, theme);
    }
    draw_overlay(frame, app, theme);
}
fn panel_block(title: &str, active: bool, theme: Theme) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if active {
            theme.border_active
        } else {
            theme.muted
        }))
        .title(title)
}
fn help(frame: &mut Frame) {
    let area = centered(frame.area(), 60, 50);
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new("j/k or arrows  Move selection\nEnter / l      Toggle detail view\n/              Filter requests\nr              Refresh asynchronously\n?              Close this help\nq              Quit").block(Block::default().borders(Borders::ALL).title(" keys ")).wrap(Wrap { trim: true }), area);
}

#[cfg(test)]
mod stabilization_tests {
    use super::*;
    use crate::model::{CiState, LoadState, ReviewState};
    use ratatui::{Terminal, backend::TestBackend};

    fn text_positions(buffer: &ratatui::buffer::Buffer, needle: &str) -> Vec<(u16, u16)> {
        let needle_width = needle.chars().count() as u16;
        let mut positions = Vec::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer
                .area
                .width
                .saturating_sub(needle_width)
                .saturating_add(1)
            {
                let mut found = String::new();
                for offset in 0..needle_width {
                    found.push_str(buffer[(x.saturating_add(offset), y)].symbol());
                }
                if found.starts_with(needle) {
                    positions.push((x, y));
                }
            }
        }
        positions
    }

    fn text_position(buffer: &ratatui::buffer::Buffer, needle: &str) -> Option<(u16, u16)> {
        text_positions(buffer, needle).into_iter().next()
    }

    #[tokio::test]
    async fn request_headers_and_selected_and_unselected_rows_share_columns() {
        let mut app = App::test_app();
        app.requests.truncate(2);
        app.requests[0].id.forge = "github".into();
        app.requests[0].id.repository = "table/repo".into();
        app.requests[0].id.number = 70;
        app.requests[0].title = "Table title".into();
        app.requests[0].ci = CiState::Failed;
        app.requests[0].review = ReviewState::Approved;
        app.requests[1].id.forge = "github".into();
        app.requests[1].id.repository = "other/repo".into();
        app.selected = 1;

        let backend = TestBackend::new(120, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                draw_list(
                    frame,
                    frame.area(),
                    &app,
                    Theme::for_mode(crate::ui::theme::ColorMode::Ansi),
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();

        let header_forge = text_position(buffer, "forge").unwrap();
        let header_repository = text_position(buffer, "repository").unwrap();
        let header_id = text_position(buffer, "id").unwrap();
        let header_title = text_position(buffer, "title").unwrap();
        let header_ci = text_position(buffer, "ci").unwrap();
        let header_review = text_position(buffer, "review").unwrap();
        let forge_positions = text_positions(buffer, "github");
        assert_eq!(forge_positions.len(), 2);
        let selected_forge = forge_positions[1];
        let repository = text_position(buffer, "table/repo").unwrap();
        let request_id = text_position(buffer, "#70").unwrap();
        let title = text_position(buffer, "Table title").unwrap();
        let ci = text_position(buffer, "✗").unwrap();
        let review = text_position(buffer, "✓").unwrap();

        assert_eq!(header_forge.0, selected_forge.0);
        assert_eq!(header_repository.0, repository.0);
        assert_eq!(header_id.0, request_id.0);
        assert_eq!(header_title.0, title.0);
        assert_eq!(header_ci.0, ci.0);
        assert_eq!(header_review.0, review.0);

        let other_forge = forge_positions[0];
        assert_eq!(selected_forge.0, other_forge.0);
        let marker = text_position(buffer, ">").unwrap();
        assert_eq!(marker.1, selected_forge.1);
        assert!(marker.0 < selected_forge.0);
    }

    #[test]
    fn request_table_widths_fit_every_terminal_width() {
        for width in 0..=240 {
            let layout = request_table_layout(width);
            let requested =
                layout.widths.iter().copied().sum::<u16>() + layout.spacing.saturating_mul(6);
            assert!(
                requested <= width.saturating_sub(2),
                "width {width}: {layout:?}"
            );
        }
    }

    #[test]
    fn live_detail_renders_loading_separately_from_loaded_empty_panels() {
        let mut app = App::test_app();
        app.demo = false;
        let id = app.requests[0].id.clone();
        app.view = View::ChangeRequestDetail(id.clone());
        let backend = TestBackend::new(120, 32);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                draw_detail(
                    frame,
                    frame.area(),
                    &app,
                    Theme::for_mode(crate::ui::theme::ColorMode::Ansi),
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert!(text_position(buffer, "Loading comments").is_some());
        assert!(text_position(buffer, "Loading CI").is_some());
        assert!(text_position(buffer, "Loading reviewers").is_some());

        let mut resources = crate::app::DetailResources::default();
        resources.request = LoadState::Loaded(());
        resources.comments = LoadState::Loaded(vec![]);
        resources.reviews = LoadState::Loaded(vec![]);
        resources.ci = LoadState::Loaded(vec![]);
        app.detail_resources.insert(id, resources);
        terminal
            .draw(|frame| {
                draw_detail(
                    frame,
                    frame.area(),
                    &app,
                    Theme::for_mode(crate::ui::theme::ColorMode::Ansi),
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert!(text_position(buffer, "No comments yet.").is_some());
        assert!(text_position(buffer, "No pipeline reported").is_some());
        assert!(text_position(buffer, "No reviewers reported").is_some());
    }

    #[test]
    fn live_dashboard_preview_does_not_claim_unstarted_detail_loads_are_running() {
        let mut app = App::test_app();
        app.demo = false;
        app.view = View::Dashboard;
        let backend = TestBackend::new(120, 32);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                draw_detail(
                    frame,
                    frame.area(),
                    &app,
                    Theme::for_mode(crate::ui::theme::ColorMode::Ansi),
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert!(text_position(buffer, "Enter to load comments").is_some());
        assert!(text_position(buffer, "Enter to load CI").is_some());
        assert!(text_position(buffer, "Enter to load reviewers").is_some());
        assert!(text_position(buffer, "Loading comments").is_none());
    }

    #[test]
    fn whitespace_only_toast_does_not_paint_a_green_notification() {
        let theme = Theme::for_mode(crate::ui::theme::ColorMode::Ansi);
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| toast(frame, "   \t", theme)).unwrap();

        assert!(
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .all(|cell| cell.bg != theme.success)
        );
    }

    #[test]
    fn request_table_handles_narrow_unicode_rows_without_panicking() {
        let mut app = App::test_app();
        app.requests.truncate(1);
        app.requests[0].id.repository = "組織/非常に長いリポジトリ名".into();
        app.requests[0].title = "🚀 Unicode title with a safely clipped tail".into();
        let backend = TestBackend::new(22, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                draw_list(
                    frame,
                    frame.area(),
                    &app,
                    Theme::for_mode(crate::ui::theme::ColorMode::Ansi),
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert!(text_position(buffer, "非常に長いリポジトリ名").is_none());
        assert!(text_position(buffer, "safely clipped tail").is_none());
    }

    #[test]
    fn request_table_uses_wide_terminals_to_preserve_repository_and_title_text() {
        let mut app = App::test_app();
        app.requests.truncate(1);
        app.requests[0].id.repository = "organization/long-project-repository".into();
        app.requests[0].title = "A long request title remains readable with room".into();
        let backend = TestBackend::new(160, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                draw_list(
                    frame,
                    frame.area(),
                    &app,
                    Theme::for_mode(crate::ui::theme::ColorMode::Ansi),
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert!(text_position(buffer, "organization/long-project-repository").is_some());
        assert!(text_position(buffer, "A long request title remains readable with room").is_some());
    }
}
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - height) / 2),
            Constraint::Percentage(height),
            Constraint::Percentage((100 - height) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - width) / 2),
            Constraint::Percentage(width),
            Constraint::Percentage((100 - width) / 2),
        ])
        .split(vertical[1])[1]
}
