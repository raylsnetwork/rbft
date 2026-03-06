// SPDX-License-Identifier: Apache-2.0
pub mod components;
pub mod render;

use anyhow::Result;
use crossterm::event::{self, Event as CEvent};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use tokio::time::interval;

use crate::automation;
use crate::automation::{spawn_script_runner, AutomationConfig, ScriptHandle};
use crate::commands::dispatch_action;
use crate::models::{AppAction, AppEvent, AppState, ValidatorTarget};

pub async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut AppState,
    rx: &mut mpsc::Receiver<AppEvent>,
    tx: mpsc::Sender<AppEvent>,
    shared_targets: Arc<RwLock<Vec<ValidatorTarget>>>,
) -> Result<()> {
    let mut automation_handle: Option<ScriptHandle> = None;
    loop {
        terminal.draw(|frame| app.draw(frame))?;

        match rx.recv().await {
            Some(AppEvent::Input(key)) => {
                match app.on_key(key) {
                    crate::models::KeyOutcome::None => {}
                    crate::models::KeyOutcome::Action(action) => {
                        let action_index = action.index();
                        app.begin_action(&action);
                        match dispatch_action(action, app, tx.clone()).await {
                            Ok(message) => {
                                app.set_status_message(crate::models::MessageKind::Info, message)
                            }
                            Err(err) => {
                                app.clear_action_state_index(action_index);
                                app.set_status_message(
                                    crate::models::MessageKind::Error,
                                    format!("Failed to issue command: {err}"),
                                );
                            }
                        }
                    }
                    crate::models::KeyOutcome::LaunchRequest(request) => {
                        // Spawn launch process in background to avoid blocking UI
                        let tx_clone = tx.clone();
                        let shared_targets_clone = shared_targets.clone();
                        let rbft_bin_dir = app.rbft_bin_dir.clone();

                        tokio::spawn(async move {
                            match crate::commands::handle_launch_chain(
                                shared_targets_clone,
                                tx_clone.clone(),
                                request,
                                rbft_bin_dir,
                            )
                            .await
                            {
                                Ok((chain, message)) => {
                                    // Send success message
                                    let _ = tx_clone
                                        .send(AppEvent::CommandResult {
                                            label: "chain".to_string(),
                                            action: "launch".to_string(),
                                            success: true,
                                            message: Some(message.clone()),
                                        })
                                        .await;

                                    // Send chain created event to update app state
                                    let _ = tx_clone.send(AppEvent::ChainCreated(chain)).await;
                                }
                                Err(err) => {
                                    let _ = tx_clone
                                        .send(AppEvent::CommandResult {
                                            label: "chain".to_string(),
                                            action: "launch".to_string(),
                                            success: false,
                                            message: Some(format!("Launch failed: {err}")),
                                        })
                                        .await;
                                }
                            }
                        });

                        app.set_status_message(
                            crate::models::MessageKind::Info,
                            "Launching chain...",
                        );
                    }
                    crate::models::KeyOutcome::Ping(idx) => {
                        match crate::commands::ping_selected_validator(app, idx).await {
                            Ok(message) => {
                                app.set_status_message(crate::models::MessageKind::Info, message)
                            }
                            Err(err) => app.set_status_message(
                                crate::models::MessageKind::Error,
                                format!("Ping failed: {err}"),
                            ),
                        }
                    }
                    crate::models::KeyOutcome::SpamRequest(request) => {
                        let shared_targets_clone = shared_targets.clone();
                        let tx_clone = tx.clone();
                        tokio::spawn(async move {
                            let validators = {
                                let targets = shared_targets_clone.read().await;
                                if targets.is_empty() {
                                    Vec::new()
                                } else {
                                    targets.iter().map(|t| t.url.to_string()).collect()
                                }
                            };
                            if validators.is_empty() {
                                let _ = tx_clone
                                    .send(AppEvent::CommandResult {
                                        label: "spam".to_string(),
                                        action: "spam".to_string(),
                                        success: false,
                                        message: Some(
                                            "No validators available for spam transactions".into(),
                                        ),
                                    })
                                    .await;
                                return;
                            }
                            if let Err(err) = crate::commands::handle_spam_request(
                                request,
                                validators,
                                tx_clone.clone(),
                            )
                            .await
                            {
                                let _ = tx_clone
                                    .send(AppEvent::CommandResult {
                                        label: "spam".to_string(),
                                        action: "spam".to_string(),
                                        success: false,
                                        message: Some(format!("Failed to start spam job: {err}")),
                                    })
                                    .await;
                            }
                        });
                        app.set_status_message(
                            crate::models::MessageKind::Info,
                            "Starting spam job...",
                        );
                    }
                    crate::models::KeyOutcome::LaunchNode(plan) => {
                        {
                            let mut targets = shared_targets.write().await;
                            targets.push(plan.target.clone());
                        }

                        let snapshot = {
                            let targets = shared_targets.read().await;
                            targets.clone()
                        };
                        let _ = tx.send(AppEvent::TargetUpdate(snapshot)).await;

                        let action = AppAction::Start(plan.entry_index);
                        app.begin_action(&action);

                        match dispatch_action(action, app, tx.clone()).await {
                            Ok(message) => {
                                app.set_status_message(crate::models::MessageKind::Info, message);
                            }
                            Err(err) => {
                                app.clear_action_state_index(plan.entry_index);
                                app.set_status_message(
                                    crate::models::MessageKind::Error,
                                    format!("Failed to launch node: {err}"),
                                );
                            }
                        }
                    }
                    crate::models::KeyOutcome::AutomationRun(run_cfg) => {
                        if automation_handle.is_some() && app.automation_is_running() {
                            app.set_status_message(
                                crate::models::MessageKind::Error,
                                "Automation is already running. Stop it before starting another.",
                            );
                            continue;
                        }
                        let Some(project_root) = app.automation_project.clone() else {
                            app.set_status_message(
                                crate::models::MessageKind::Error,
                                "No automation project configured",
                            );
                            continue;
                        };
                        let config = AutomationConfig {
                            project_root,
                            script_path: run_cfg.script.relative_path.clone(),
                            contract: run_cfg.script.contract.clone(),
                        };
                        match spawn_script_runner(config, tx.clone()) {
                            Ok(handle) => {
                                automation_handle = Some(handle);
                                app.set_status_message(
                                    crate::models::MessageKind::Info,
                                    "Launching automation script...",
                                );
                            }
                            Err(err) => {
                                app.set_status_message(
                                    crate::models::MessageKind::Error,
                                    format!("Failed to launch automation: {err}"),
                                );
                            }
                        }
                    }
                    crate::models::KeyOutcome::AutomationStop => {
                        if let Some(handle) = automation_handle.as_ref() {
                            handle.cancel();
                            app.automation_push_log("Stop requested...");
                            app.set_status_message(
                                crate::models::MessageKind::Info,
                                "Stopping automation script...",
                            );
                        } else {
                            app.set_status_message(
                                crate::models::MessageKind::Info,
                                "No automation script is running",
                            );
                        }
                    }
                }
            }
            Some(AppEvent::Data(reports)) => {
                let mut had_error = None;
                for rep in &reports {
                    if !rep.ok {
                        had_error = rep.error.clone();
                        break;
                    }
                }
                if let Some(err) = had_error {
                    app.set_data_status_message(crate::models::MessageKind::Error, err);
                } else {
                    app.clear_error_message();
                }
                app.apply_reports(reports);
            }
            Some(AppEvent::CommandResult {
                label,
                action,
                success,
                message,
            }) => {
                app.handle_command_result(&label, &action, success, message);
            }
            Some(AppEvent::ChainCreated(chain)) => {
                app.set_active_chain(chain.clone());
                app.set_status_message(
                    crate::models::MessageKind::Info,
                    format!(
                        "Chain launched: {} validators, base port {}. Use 'n' key to start \
                         validators.",
                        chain.num_nodes, chain.base_http_port
                    ),
                );
            }
            Some(AppEvent::TargetUpdate(targets)) => {
                app.set_targets(targets);
            }
            Some(AppEvent::AutomationStarted(selection)) => {
                app.automation_start(format!(
                    "{} ({})",
                    selection.contract, selection.relative_path
                ));
                app.set_status_message(
                    crate::models::MessageKind::Info,
                    format!("Automation script {} started", selection.contract),
                );
            }
            Some(AppEvent::AutomationLog(line)) => {
                app.automation_push_log(line);
            }
            Some(AppEvent::AutomationStopped { success, message }) => {
                app.automation_stop(message.clone());
                if let Some(handle) = automation_handle.take() {
                    handle.detach();
                }
                let kind = if success {
                    crate::models::MessageKind::Info
                } else {
                    crate::models::MessageKind::Error
                };
                app.set_status_message(kind, message);
            }
            Some(AppEvent::Automation(action)) => {
                if let Err(err) = automation::handle_automation_action(
                    action,
                    app,
                    tx.clone(),
                    shared_targets.clone(),
                )
                .await
                {
                    app.set_status_message(
                        crate::models::MessageKind::Error,
                        format!("Automation error: {err}"),
                    );
                }
            }
            Some(AppEvent::Tick) => {}
            None => break,
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

pub async fn spawn_input(tx: mpsc::Sender<AppEvent>) {
    loop {
        if event::poll(Duration::from_millis(100)).unwrap_or(false) {
            if let Ok(CEvent::Key(key)) = event::read() {
                if tx.send(AppEvent::Input(key)).await.is_err() {
                    break;
                }
            }
        }
    }
}

pub async fn spawn_ticks(tx: mpsc::Sender<AppEvent>) {
    let mut ticker = interval(Duration::from_millis(200));
    loop {
        ticker.tick().await;
        if tx.send(AppEvent::Tick).await.is_err() {
            break;
        }
    }
}

pub fn cleanup_terminal(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>) -> Result<()> {
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        terminal.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    Ok(())
}
