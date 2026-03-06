// SPDX-License-Identifier: Apache-2.0
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap};
use ratatui::Frame;

use crate::models::{AppState, MessageKind};
use std::time::Instant;

pub fn draw_header(frame: &mut Frame<'_>, area: Rect) {
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            " validator-inspector ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" — Interactive RBFT Validator Monitor"),
    ]))
    .block(Block::default().borders(Borders::ALL));

    frame.render_widget(title, area);
}

pub fn draw_table(frame: &mut Frame<'_>, app: &mut AppState, area: Rect, now: Instant) {
    let header = Row::new(vec![
        Cell::from("Validator"),
        Cell::from("Status"),
        Cell::from("Height"),
        Cell::from("Txs"),
        Cell::from("Port"),
        Cell::from("Key"),
    ])
    .style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );

    let rows = app.entries.iter().map(|entry| {
        let status = entry.status_class(now);
        let (status_text, status_style) = if let Some(lifecycle) = entry.lifecycle {
            (lifecycle.text().to_string(), lifecycle.style())
        } else {
            (status.text().to_string(), status.style())
        };
        Row::new(vec![
            Cell::from(entry.target.label.clone()),
            Cell::from(status_text).style(status_style),
            Cell::from(entry.height_str()),
            Cell::from(entry.txs_str()),
            Cell::from(
                entry
                    .target
                    .http_port()
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "-".into()),
            ),
            Cell::from(entry.target.key.clone().unwrap_or_else(|| "-".into())),
        ])
    });

    let widths = [
        Constraint::Length(12),
        Constraint::Length(10),
        Constraint::Length(12),
        Constraint::Length(12),
        Constraint::Length(10),
        Constraint::Length(18),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title("Validators"))
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    frame.render_stateful_widget(table, area, &mut app.table_state);
}

pub fn draw_footer(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let mut spans = vec![Span::styled(
        "Shortcuts: ",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )];
    let can_start = app.control.can_start() || app.active_chain.is_some();
    spans.push(Span::raw("[l] Launch "));
    spans.push(Span::raw("[m] Add node "));
    spans.push(Span::raw("[t] Spam "));
    spans.push(Span::raw("[a] Automation "));
    if can_start {
        spans.push(Span::raw("[n] Start "));
    }
    spans.push(Span::raw("[k] Kill "));
    spans.push(Span::raw("[p] Ping "));
    spans.push(Span::raw("[Enter] Logs "));
    if app.automation_is_running() {
        spans.push(Span::raw("[x] Stop script "));
    }
    spans.extend([
        Span::raw("[↑/↓] Navigate "),
        Span::raw("[PgUp/PgDn] Scroll "),
        Span::raw("[q/Q/Esc] Quit "),
    ]);
    if let Some((kind, msg)) = &app.status_message {
        spans.push(Span::raw(" | "));
        let style = match kind {
            MessageKind::Info => Style::default().fg(Color::Cyan),
            MessageKind::Error => Style::default().fg(Color::Red),
        };
        // Flatten newlines so multi-line status (like spam output) shows inline
        let flattened = msg.replace('\n', " | ").replace('\r', "");
        spans.push(Span::styled(flattened, style));
    }
    let footer = Paragraph::new(Line::from(spans))
        .block(Block::default().borders(Borders::ALL))
        .wrap(Wrap { trim: true });
    frame.render_widget(footer, area);
}
