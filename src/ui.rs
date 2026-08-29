use crate::app::App;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

const ACCENT: Color = Color::Cyan;
pub fn draw(frame: &mut Frame, app: &App) {
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
                .fg(Color::Black)
                .bg(ACCENT)
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
    draw_list(frame, panes[0], app);
    draw_detail(frame, panes[1], app);
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
        .style(Style::default().fg(Color::DarkGray)),
        outer[2],
    );
    if app.show_help {
        help(frame);
    }
}
fn draw_list(frame: &mut Frame, area: Rect, app: &App) {
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
                        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
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
        List::new(items).block(Block::default().borders(Borders::ALL).title(title)),
        area,
    );
}
fn draw_detail(frame: &mut Frame, area: Rect, app: &App) {
    let split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);
    let Some(pr) = app.selected_request() else {
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
        .take(10)
        .map(|c| {
            Line::from(format!(
                "{}: {}",
                c.author.name.as_deref().unwrap_or(&c.author.login),
                c.body
            ))
        })
        .collect::<Vec<_>>();
    let mut left = vec![
        Line::styled(&pr.title, Style::default().add_modifier(Modifier::BOLD)),
        Line::from(format!(
            "{} · {} → {} · +{} / -{}",
            pr.id.display(pr.kind),
            pr.source_branch,
            pr.target_branch,
            pr.additions,
            pr.deletions
        )),
        Line::from(""),
        Line::styled("Latest comments", Style::default().fg(ACCENT)),
    ];
    left.extend(comments);
    frame.render_widget(
        Paragraph::new(left).wrap(Wrap { trim: true }).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" selected request "),
        ),
        split[0],
    );
    let mut right = vec![Line::styled(
        "CI",
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
    )];
    if let Some(pipeline) = &pr.pipeline {
        for job in &pipeline.jobs {
            right.push(Line::from(format!(
                "{} {:<24} {}",
                job.status.glyph(),
                job.name,
                job.duration_seconds
                    .map(|s| format!("{s}s"))
                    .unwrap_or_else(|| job.status.label().into())
            )));
        }
    }
    right.extend([
        Line::from(""),
        Line::styled(
            "Reviewers",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
    ]);
    for reviewer in &pr.reviewers {
        right.push(Line::from(format!(
            "{} {:<16} {}",
            reviewer.state.glyph(),
            reviewer
                .person
                .name
                .as_deref()
                .unwrap_or(&reviewer.person.login),
            reviewer.state.label()
        )));
    }
    frame.render_widget(
        Paragraph::new(right).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" CI / review "),
        ),
        split[1],
    );
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
