// SPDX-License-Identifier: Apache-2.0
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::Frame;
use std::time::Instant;

use super::components::{
    draw_automation_modal, draw_automation_panel, draw_footer, draw_header, draw_launch_modal,
    draw_log_modal, draw_log_panel, draw_receipts_panel, draw_spam_modal, draw_stats_panel,
    draw_table,
};
use crate::models::AppState;

impl AppState {
    pub fn draw(&mut self, frame: &mut Frame<'_>) {
        let now = Instant::now();
        let size = frame.area();
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints(
                [
                    Constraint::Length(3),
                    Constraint::Min(5),
                    Constraint::Length(3),
                ]
                .as_ref(),
            )
            .split(size);

        draw_header(frame, layout[0]);

        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)].as_ref())
            .split(layout[1]);

        let left = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)].as_ref())
            .split(body[0]);

        let right = Layout::default()
            .direction(Direction::Vertical)
            .constraints(
                [
                    Constraint::Percentage(55),
                    Constraint::Percentage(20),
                    Constraint::Percentage(25),
                ]
                .as_ref(),
            )
            .split(body[1]);

        draw_table(frame, self, left[0], now);
        draw_log_panel(frame, left[1], self);
        draw_receipts_panel(frame, right[0], self);
        draw_stats_panel(frame, right[1], self);
        draw_automation_panel(frame, right[2], self);

        draw_footer(frame, self, layout[2]);

        if let Some(form) = &self.launch_form {
            let popup = centered_rect(60, 50, frame.area());
            draw_launch_modal(frame, popup, form);
        }
        if let Some(form) = &self.spam_form {
            let popup = centered_rect(70, 60, frame.area());
            draw_spam_modal(frame, popup, form);
        }
        if let Some(modal) = self.log_modal.as_mut() {
            let popup = centered_rect(80, 70, frame.area());
            draw_log_modal(frame, popup, modal);
        }
        if let Some(modal) = &self.automation_modal {
            let popup = centered_rect(60, 50, frame.area());
            draw_automation_modal(
                frame,
                popup,
                modal,
                self.automation_project_label().as_deref(),
            );
        }
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Percentage((100 - percent_y) / 2),
                Constraint::Percentage(percent_y),
                Constraint::Percentage((100 - percent_y) / 2),
            ]
            .as_ref(),
        )
        .split(area);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            [
                Constraint::Percentage((100 - percent_x) / 2),
                Constraint::Percentage(percent_x),
                Constraint::Percentage((100 - percent_x) / 2),
            ]
            .as_ref(),
        )
        .split(vertical[1]);
    horizontal[1]
}
