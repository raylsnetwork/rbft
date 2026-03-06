// SPDX-License-Identifier: Apache-2.0
use tokio::sync::mpsc;
use tokio::time::{sleep_until, Instant};
use tokio_util::sync::CancellationToken;

use crate::models::AppEvent;

use super::ScheduledCommand;

pub async fn run(
    commands: Vec<ScheduledCommand>,
    tx: mpsc::Sender<AppEvent>,
    token: CancellationToken,
) {
    if commands.is_empty() {
        return;
    }

    let start = Instant::now();
    let mut tasks = Vec::with_capacity(commands.len());

    for command in commands {
        let tx_clone = tx.clone();
        let child = token.child_token();
        tasks.push(tokio::spawn(async move {
            let ScheduledCommand {
                action,
                offset,
                interval,
                repeats,
            } = command;
            let mut remaining = repeats;
            let mut next_trigger = start + offset;
            loop {
                if child.is_cancelled() {
                    break;
                }
                let now = Instant::now();
                if next_trigger > now {
                    tokio::select! {
                        _ = child.cancelled() => break,
                        _ = sleep_until(next_trigger) => {}
                    }
                }
                if child.is_cancelled() {
                    break;
                }
                if tx_clone
                    .send(AppEvent::Automation(action.clone()))
                    .await
                    .is_err()
                {
                    break;
                }
                if let Some(interval) = interval {
                    if remaining == 0 {
                        break;
                    }
                    remaining -= 1;
                    next_trigger += interval;
                } else {
                    break;
                }
            }
        }));
    }

    for task in tasks {
        let _ = task.await;
    }
}
