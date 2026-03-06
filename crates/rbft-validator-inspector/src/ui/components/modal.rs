// SPDX-License-Identifier: Apache-2.0
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::models::LogModal;

pub fn draw_log_modal(frame: &mut Frame<'_>, area: Rect, modal: &mut LogModal) {
    // Clear the entire popup area first
    frame.render_widget(Clear, area);

    let lines: Vec<Line> = modal
        .lines
        .iter()
        .map(|line| Line::from(line.clone()))
        .collect();

    let inner_height = area.height.saturating_sub(2); // Account for borders
    let content_height = lines.len() as u16;
    let max_scroll = content_height.saturating_sub(inner_height);

    if modal.follow_tail || modal.scroll > max_scroll {
        modal.scroll = max_scroll;
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .title(modal.title.clone())
        .border_style(Style::default().fg(Color::Yellow));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((modal.scroll, 0));

    frame.render_widget(paragraph, area);
}
