// SPDX-License-Identifier: Apache-2.0
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::models::{AutomationModal, LaunchForm, SpamField, SpamForm, SpamMode};

pub fn draw_launch_modal(frame: &mut Frame<'_>, area: Rect, form: &LaunchForm) {
    frame.render_widget(Clear, area);

    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        "Configure Testnet Launch",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    for (idx, field) in form.fields.iter().enumerate() {
        let label = field.label();
        let value = &form.inputs[idx];

        let spans = if idx == form.index {
            // Highlight current field
            vec![
                Span::styled("> ", Style::default().fg(Color::Cyan)),
                Span::styled(
                    format!("{label}: "),
                    Style::default()
                        .fg(Color::LightBlue)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(value.clone(), Style::default().add_modifier(Modifier::BOLD)),
                Span::styled("█", Style::default().fg(Color::Yellow)), // Cursor indicator
            ]
        } else {
            vec![
                Span::styled(format!("{label}: "), Style::default().fg(Color::LightBlue)),
                Span::raw(value.clone()),
            ]
        };

        lines.push(Line::from(spans));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::raw(
        "Enter=Next/Submit  •  Esc=Cancel  •  Tab/↑/↓=Switch field",
    )));

    if let Some(err) = &form.error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("Error: {err}"),
            Style::default().fg(Color::Red),
        )));
    }

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Launch Testnet"),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, area);
}

pub fn draw_spam_modal(frame: &mut Frame<'_>, area: Rect, form: &SpamForm) {
    frame.render_widget(Clear, area);

    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        "Spam Transactions",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from("Configure transaction spam parameters"));
    lines.push(Line::from(""));

    for (idx, field) in form.fields.iter().enumerate() {
        let mut value = form.inputs[idx].clone();
        if matches!(field, SpamField::Mode) {
            value = form.mode().label().to_string();
        }

        let mut spans = if idx == form.index {
            vec![
                Span::styled("> ", Style::default().fg(Color::Cyan)),
                Span::styled(
                    format!("{}: ", field.label()),
                    Style::default()
                        .fg(Color::LightBlue)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(value, Style::default().add_modifier(Modifier::BOLD)),
                Span::styled("█", Style::default().fg(Color::Yellow)),
            ]
        } else {
            vec![
                Span::styled(
                    format!("{}: ", field.label()),
                    Style::default().fg(Color::LightBlue),
                ),
                Span::raw(value),
            ]
        };

        if matches!(field, SpamField::TargetUrl) && !matches!(form.mode(), SpamMode::Target) {
            spans.push(Span::styled(
                " (unused for round-robin)",
                Style::default().fg(Color::DarkGray),
            ));
        }

        lines.push(Line::from(spans));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::raw(
        "Enter=Next/Submit  •  Esc=Cancel  •  Tab/↑/↓=Switch field",
    )]));
    lines.push(Line::from(vec![Span::raw(
        "Mode options: round-robin or target",
    )]));

    if let Some(err) = &form.error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("Error: {err}"),
            Style::default().fg(Color::Red),
        )));
    }

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Spam Transactions"),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, area);
}

pub fn draw_automation_modal(
    frame: &mut Frame<'_>,
    area: Rect,
    modal: &AutomationModal,
    project_root: Option<&str>,
) {
    frame.render_widget(Clear, area);

    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        "Run Automation Script",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from("Select a script to execute"));
    if let Some(root) = project_root {
        lines.push(Line::from(vec![
            Span::styled("Scripts folder: ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{root}/contracts/scripts")),
        ]));
    } else {
        lines.push(Line::from(Span::styled(
            "No automation project configured",
            Style::default().fg(Color::DarkGray),
        )));
    }
    lines.push(Line::from(""));

    if modal.scripts.is_empty() {
        let hint = if let Some(root) = project_root {
            format!("No automation scripts found in {root}/contracts/scripts")
        } else {
            "No automation scripts discovered".to_string()
        };
        lines.push(Line::from(Span::styled(
            hint,
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for (idx, script) in modal.scripts.iter().enumerate() {
            let mut spans = Vec::new();
            if idx == modal.index {
                spans.push(Span::styled("> ", Style::default().fg(Color::Cyan)));
                spans.push(Span::styled(
                    script.display_name(),
                    Style::default()
                        .fg(Color::LightBlue)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::raw("  "));
                spans.push(Span::raw(script.display_name()));
            }
            lines.push(Line::from(spans));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::raw(
        "Enter=Run  •  Esc=Cancel  •  ↑/↓/j/k=Navigate",
    )));

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Automation Scripts"),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, area);
}
