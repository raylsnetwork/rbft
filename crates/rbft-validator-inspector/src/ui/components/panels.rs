// SPDX-License-Identifier: Apache-2.0
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};
use ratatui::Frame;

use crate::models::AppState;

pub fn draw_log_panel(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let title = match app.inline_log_label() {
        Some(label) => format!("Logs — {label}"),
        None => "Logs".to_string(),
    };

    let block = Block::default().borders(Borders::ALL).title(title);
    let lines: Vec<Line> = if !app.inline_logs_enabled() {
        vec![Line::from(Span::styled(
            "Inline logs disabled",
            Style::default().fg(Color::DarkGray),
        ))]
    } else if let Some(error) = app.inline_log_error() {
        vec![Line::from(Span::styled(
            error.to_string(),
            Style::default().fg(Color::Red),
        ))]
    } else if app.inline_log_lines().is_empty() {
        vec![Line::from("No log data available")]
    } else {
        app.inline_log_lines()
            .iter()
            .map(|line| Line::from(line.clone()))
            .collect()
    };

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

pub fn draw_receipts_panel(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Transactions ([ / ] to scroll)");
    let entries = app.receipt_entries();
    let mut lines: Vec<Line> = if entries.is_empty() {
        vec![Line::from(
            "No transactions recorded for this validator yet.",
        )]
    } else {
        entries
            .iter()
            .map(|receipt| {
                let status_span = match receipt.status {
                    Some(true) => Span::styled("OK", Style::default().fg(Color::Green)),
                    Some(false) => Span::styled("FAIL", Style::default().fg(Color::Red)),
                    None => Span::raw("UNK"),
                };
                let block = receipt
                    .block_number
                    .map(|b| b.to_string())
                    .unwrap_or_else(|| "-".into());
                let gas = receipt
                    .gas_used
                    .map(|g| g.to_string())
                    .unwrap_or_else(|| "-".into());
                let from = receipt
                    .from
                    .as_deref()
                    .map(shorten_id)
                    .unwrap_or_else(|| "-".into());
                let to = receipt
                    .to
                    .as_deref()
                    .map(shorten_id)
                    .unwrap_or_else(|| "-".into());
                Line::from(vec![
                    Span::styled(
                        format!("{} ", receipt.label),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    status_span,
                    Span::raw(format!(
                        "  #{block}  gas {gas}  {} → {}  tx {}",
                        from, to, receipt.tx_hash
                    )),
                ])
            })
            .collect()
    };

    let inner_height = area.height.saturating_sub(2);
    if entries.len() > inner_height as usize && inner_height > 0 {
        lines.push(Line::from(Span::styled(
            "Use [ and ] to scroll",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let content_height = lines.len() as u16;
    let max_scroll = content_height.saturating_sub(inner_height);
    let scroll = app.receipts_scroll.min(max_scroll);

    let paragraph = Paragraph::new(lines)
        .block(block.clone())
        .wrap(Wrap { trim: true })
        .scroll((scroll, 0));
    frame.render_widget(paragraph, area);

    if content_height > inner_height && inner_height > 0 {
        let mut scrollbar_state = ScrollbarState::new(
            content_height
                .saturating_sub(inner_height)
                .saturating_add(1) as usize,
        )
        .position(scroll as usize);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"))
            .track_symbol(Some("│"))
            .thumb_symbol("█");
        frame.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
    }
}

pub fn draw_stats_panel(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let stats = app.chain_stats();
    let block = Block::default().borders(Borders::ALL).title("Chain Stats");

    let lines = vec![
        stat_line(
            "Uptime",
            stats
                .uptime
                .map(format_duration)
                .unwrap_or_else(|| "-".into()),
        ),
        stat_line(
            "Validators",
            format!(
                "{} total / {} active",
                stats.total_validators, stats.active_validators
            ),
        ),
        stat_line(
            "Highest Block",
            stats
                .highest_block
                .map(|h| h.to_string())
                .unwrap_or_else(|| "-".into()),
        ),
        stat_line("Total TXs", stats.total_txs.to_string()),
        stat_line("Avg Latency", format_latency(stats.average_latency)),
    ];

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });
    frame.render_widget(paragraph, area);
}

pub fn draw_automation_panel(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let mut title = String::from("Automation");
    if let Some(label) = app.automation_script_label() {
        title.push_str(&format!(" — {}", label));
    }
    let block = Block::default().borders(Borders::ALL).title(title);
    let max_width = area.width.saturating_sub(2) as usize; // leave space for borders

    let mut lines: Vec<Line> = Vec::new();
    if app.automation_is_running() {
        lines.push(Line::from(Span::styled(
            "Script running (press [x] to stop)",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
    } else {
        lines.push(Line::from(Span::styled(
            "No automation script running",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(""));
    }

    if app.automation_log_lines().is_empty() {
        lines.push(Line::from("No automation logs"));
    } else {
        for entry in app.automation_log_lines() {
            let display = if max_width == 0 {
                String::new()
            } else if entry.chars().count() > max_width {
                // Truncate long lines to keep them inside the panel boundaries
                let mut s: String = entry.chars().take(max_width.saturating_sub(1)).collect();
                s.push('…');
                s
            } else {
                entry.clone()
            };
            let style = if entry.contains("error") || entry.contains("Error") {
                Style::default().fg(Color::Red)
            } else if entry.contains("Loaded")
                || entry.contains("Starting")
                || entry.contains("Launching")
                || entry.contains("plan")
            {
                Style::default().fg(Color::LightGreen)
            } else if entry.contains("Compil") || entry.contains("forge build") {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(display, style)));
        }
    }

    let inner_height = area.height.saturating_sub(2);
    let content_height = lines.len() as u16;
    let scroll = content_height.saturating_sub(inner_height);

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: true })
        .scroll((scroll, 0));
    frame.render_widget(paragraph, area);
}

fn stat_line(label: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label}: "),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(value),
    ])
}

fn format_duration(duration: std::time::Duration) -> String {
    let secs = duration.as_secs();
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else if seconds > 0 {
        format!("{seconds}s")
    } else {
        let millis = duration.as_millis();
        format!("{millis}ms")
    }
}

fn format_latency(value: Option<std::time::Duration>) -> String {
    match value {
        Some(duration) => {
            if duration.as_millis() > 0 {
                format!("{:.1} ms", duration.as_secs_f64() * 1000.0)
            } else {
                format!("{:.3} ms", duration.as_secs_f64() * 1000.0)
            }
        }
        None => "-".into(),
    }
}

fn shorten_id(id: &str) -> String {
    if id.len() <= 12 {
        id.to_string()
    } else {
        format!("{}…{}", &id[..6], &id[id.len().saturating_sub(4)..])
    }
}
