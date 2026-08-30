pub mod theme;
use crate::app::{App, DetailFocus, HitRegions, Overlay, View};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

use theme::Theme;
pub fn draw(frame: &mut Frame, app: &mut App) {
    let theme = Theme::detect();
    if matches!(app.view, View::ChangeRequestDetail(_)) {
        draw_full_detail(frame, app, theme);
        if let Some(overlay) = &app.overlay {
            overlay_view(frame, overlay, theme);
        }
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
            " / filter  r refresh  Enter detail  ? keys  q quit   {health}"
        ))
        .style(Style::default().fg(theme.muted)),
        outer[2],
    );
    if app.show_help {
        help(frame);
    }
    if let Some(message) = &app.toast {
        toast(frame, message, theme);
    }
    if let Some(overlay) = &app.overlay {
        overlay_view(frame, overlay, theme);
    }
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
        Paragraph::new(" ↑↓ scroll  PgUp/PgDn fast  Tab panel  c comment  R review  Esc back")
            .style(Style::default().fg(theme.muted)),
        outer[2],
    );
}
fn toast(frame: &mut Frame, message: &str, theme: Theme) {
    let area = Rect::new(
        frame.area().x.saturating_add(2),
        frame.area().bottom().saturating_sub(3),
        (message.len() as u16 + 6).min(frame.area().width.saturating_sub(4)),
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
fn overlay_view(frame: &mut Frame, overlay: &Overlay, theme: Theme) {
    let area = centered(frame.area(), 62, 42);
    frame.render_widget(Clear, area);
    let (title, body) = match overlay {
        Overlay::Composer { body } => (
            "Add comment",
            format!("{}\n\nCtrl+Enter submits · Esc cancels", body),
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
        Overlay::Palette { query, selected } => {
            let commands = [
                "Add comment",
                "Approve",
                "Request changes",
                "Refresh",
                "Request reviewer",
                "Open in browser",
            ];
            (
                "Command palette",
                format!(
                    "{}\n\n{}",
                    query,
                    commands
                        .iter()
                        .enumerate()
                        .filter(|(_, command)| command
                            .to_lowercase()
                            .contains(&query.to_lowercase()))
                        .map(|(index, command)| format!(
                            "{} {command}",
                            if index == *selected { ">" } else { " " }
                        ))
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
            )
        }
        Overlay::ConfirmDelete => (
            "Delete comment",
            "Delete this comment?\n\nEnter / Esc cancel    d delete".into(),
        ),
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
    let items: Vec<ListItem> = visible
        .iter()
        .enumerate()
        .map(|(i, pr)| {
            let marker = if i == app.selected { ">" } else { " " };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!(
                        "{marker} {:<12} {:<20} {:>4}  ",
                        pr.id.forge,
                        pr.id.repository,
                        pr.id.display(pr.kind)
                    ),
                    if i == app.selected {
                        Style::default()
                            .fg(theme.selection_fg)
                            .bg(theme.selection_bg)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    },
                ),
                Span::raw(format!(
                    "{:<30.30} {} {:<8} {} {}",
                    pr.title,
                    pr.ci.glyph(),
                    pr.ci.label(),
                    pr.review.glyph(),
                    pr.review.label()
                )),
            ]))
        })
        .collect();
    let title = if app.filtering {
        format!(" requests  filter: {}_ ", app.filter)
    } else if app.filter.is_empty() {
        " requests  forge        repository              id    title                          ci       review ".into()
    } else {
        format!(" requests  filter: {} ", app.filter)
    };
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border_active))
                .title(title),
        ),
        area,
    );
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
    let comments = pr
        .comments
        .iter()
        .rev()
        .skip(app.comment_scroll)
        .take(10)
        .map(|c| {
            Line::from(format!(
                "{} · {}{}\n{}",
                c.author.name.as_deref().unwrap_or(&c.author.login),
                c.created_at.format("%H:%M"),
                if c.updated_at.is_some() {
                    " edited"
                } else {
                    ""
                },
                c.body.replace('\n', " ")
            ))
        })
        .collect::<Vec<_>>();
    let description = vec![
        Line::styled(&pr.title, Style::default().add_modifier(Modifier::BOLD)),
        Line::from(format!(
            "{} · {} → {}",
            pr.id.display(pr.kind),
            pr.source_branch,
            pr.target_branch
        )),
        Line::from(format!(
            "+{} / -{} · {}",
            pr.additions,
            pr.deletions,
            pr.mergeability.label()
        )),
    ];
    frame.render_widget(
        Paragraph::new(description).block(panel_block(
            " description ",
            app.detail_focus == DetailFocus::Description,
            theme,
        )),
        left[0],
    );
    let mut comment_lines = vec![Line::styled(
        format!("latest {} of {}", comments.len(), pr.comments.len()),
        Style::default().fg(theme.secondary),
    )];
    comment_lines.extend(comments);
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
    let mut ci = vec![];
    if let Some(pipeline) = &pr.pipeline {
        for job in pipeline.jobs.iter().skip(app.ci_scroll).take(8) {
            ci.push(Line::from(format!(
                "{} {:<24} {}",
                job.status.glyph(),
                job.name,
                job.duration_seconds
                    .map(|s| format!("{s}s"))
                    .unwrap_or_else(|| job.status.label().into())
            )));
        }
    } else {
        ci.push(Line::from("No pipeline reported"));
    }
    frame.render_widget(
        Paragraph::new(ci).block(panel_block(
            " CI ",
            app.detail_focus == DetailFocus::Ci,
            theme,
        )),
        right[0],
    );
    let reviewers = pr
        .reviewers
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
        .collect::<Vec<_>>();
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
