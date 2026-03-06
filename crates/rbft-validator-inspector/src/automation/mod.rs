// SPDX-License-Identifier: Apache-2.0
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use tokio::sync::{mpsc, RwLock};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::commands::{
    dispatch_action, handle_launch_chain, handle_spam_request, ping_selected_validator,
};
use crate::models::{
    AppAction, AppEvent, AppState, AutomationAction, AutomationScriptSelection, LaunchRequest,
    MessageKind, SpamRequest, ValidatorTarget,
};

mod compiler;
mod runner;

pub use compiler::{AutomationConfig, ScheduledCommand};

pub struct ScriptHandle {
    cancel: CancellationToken,
    join: JoinHandle<()>,
}

impl ScriptHandle {
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub fn detach(self) {
        tokio::spawn(async move {
            let _ = self.join.await;
        });
    }
}

pub fn spawn_script_runner(
    config: AutomationConfig,
    tx: mpsc::Sender<AppEvent>,
) -> Result<ScriptHandle> {
    let cancel = CancellationToken::new();
    let runner = ScriptRunner {
        config,
        tx: tx.clone(),
        cancel: cancel.clone(),
    };
    let join = tokio::spawn(async move {
        if let Err(err) = runner.run().await {
            let _ = tx
                .send(AppEvent::AutomationLog(format!("Automation error: {err}")))
                .await;
            let _ = tx
                .send(AppEvent::AutomationStopped {
                    success: false,
                    message: format!("Automation failed: {err}"),
                })
                .await;
        }
    });
    Ok(ScriptHandle { cancel, join })
}

struct ScriptRunner {
    config: AutomationConfig,
    tx: mpsc::Sender<AppEvent>,
    cancel: CancellationToken,
}

impl ScriptRunner {
    async fn run(self) -> Result<()> {
        let selection = AutomationScriptSelection {
            relative_path: self.config.script_path.clone(),
            contract: self.config.contract.clone(),
        };
        self.tx
            .send(AppEvent::AutomationStarted(selection.clone()))
            .await
            .ok();

        self.log(format!(
            "Compiling {} ({})",
            selection.contract, selection.relative_path
        ))
        .await;
        let build_output = compiler::run_forge_build(&self.config.project_root).await?;
        if !build_output.is_empty() {
            self.log(build_output).await;
        }
        if self.cancel.is_cancelled() {
            self.log("Automation cancelled before execution").await;
            self.tx
                .send(AppEvent::AutomationStopped {
                    success: false,
                    message: "Automation cancelled".into(),
                })
                .await
                .ok();
            return Ok(());
        }

        let commands = compiler::load_commands(&self.config).await?;
        if commands.is_empty() {
            self.log("No automation commands found in script").await;
            self.tx
                .send(AppEvent::AutomationStopped {
                    success: false,
                    message: "Automation script produced no commands".into(),
                })
                .await
                .ok();
            return Ok(());
        }
        self.log(format!("Loaded {} automation commands", commands.len()))
            .await;
        runner::run(commands, self.tx.clone(), self.cancel.clone()).await;
        if self.cancel.is_cancelled() {
            self.log("Automation cancelled").await;
            self.tx
                .send(AppEvent::AutomationStopped {
                    success: false,
                    message: "Automation cancelled".into(),
                })
                .await
                .ok();
        } else {
            self.log("Automation script completed").await;
            self.tx
                .send(AppEvent::AutomationStopped {
                    success: true,
                    message: "Automation completed".into(),
                })
                .await
                .ok();
        }
        Ok(())
    }

    async fn log(&self, line: impl Into<String>) {
        let text = line.into();
        if text.contains('\n') {
            for part in text.split('\n') {
                let part = part.trim_end();
                if !part.is_empty() {
                    let _ = self
                        .tx
                        .send(AppEvent::AutomationLog(part.to_string()))
                        .await;
                }
            }
        } else {
            let _ = self.tx.send(AppEvent::AutomationLog(text)).await;
        }
    }
}

pub async fn handle_automation_action(
    action: AutomationAction,
    app: &mut AppState,
    tx: mpsc::Sender<AppEvent>,
    shared_targets: Arc<RwLock<Vec<ValidatorTarget>>>,
) -> Result<()> {
    match action {
        AutomationAction::LaunchChain(request) => {
            app.automation_push_log("Launching chain from automation script");
            let rbft_bin_dir = app.rbft_bin_dir.clone();
            spawn_chain_launch(
                request,
                rbft_bin_dir,
                app,
                tx.clone(),
                shared_targets.clone(),
            )
            .await;
        }
        AutomationAction::StartValidator(label) => {
            app.automation_push_log(format!("Starting validator {label}"));
            run_lifecycle_action(AppAction::Start, app, tx.clone(), &label).await?;
        }
        AutomationAction::StopValidator(label) => {
            app.automation_push_log(format!("Stopping validator {label}"));
            run_lifecycle_action(AppAction::Stop, app, tx.clone(), &label).await?;
        }
        AutomationAction::RestartValidator(label) => {
            app.automation_push_log(format!("Restarting validator {label}"));
            run_lifecycle_action(AppAction::Restart, app, tx.clone(), &label).await?;
        }
        AutomationAction::LaunchValidator { auto_start } => {
            app.automation_push_log("Launching additional validator");
            launch_additional_validator(auto_start, app, tx.clone(), shared_targets).await?;
        }
        AutomationAction::SpamJob(request) => {
            app.automation_push_log("Starting spam job from automation script");
            spawn_spam_job(request, app, tx.clone(), shared_targets).await?;
        }
        AutomationAction::PingValidator(label) => match app.index_for_label(&label) {
            Some(idx) => match ping_selected_validator(app, idx).await {
                Ok(message) => {
                    app.automation_push_log(message.clone());
                    app.set_status_message(MessageKind::Info, message)
                }
                Err(err) => app.set_status_message(
                    MessageKind::Error,
                    format!("Automation ping failed for {}: {}", label, err),
                ),
            },
            None => app.set_status_message(
                MessageKind::Error,
                format!("Automation ping skipped, no validator {}", label),
            ),
        },
        AutomationAction::Log(message) => {
            app.automation_push_log(message);
        }
    }
    Ok(())
}

// existing helper functions spawn_chain_launch, run_lifecycle_action, etc. (keep as before)

async fn spawn_chain_launch(
    request: LaunchRequest,
    rbft_bin_dir: PathBuf,
    app: &mut AppState,
    tx: mpsc::Sender<AppEvent>,
    shared_targets: Arc<RwLock<Vec<ValidatorTarget>>>,
) {
    let tx_clone = tx.clone();
    let shared_clone = shared_targets.clone();
    tokio::spawn(async move {
        match handle_launch_chain(
            shared_clone,
            tx_clone.clone(),
            request.clone(),
            rbft_bin_dir.clone(),
        )
        .await
        {
            Ok((chain, message)) => {
                let _ = tx_clone
                    .send(AppEvent::AutomationLog(format!(
                        "Chain launch succeeded: {message}"
                    )))
                    .await;
                let _ = tx_clone
                    .send(AppEvent::CommandResult {
                        label: "chain".to_string(),
                        action: "launch".to_string(),
                        success: true,
                        message: Some(message.clone()),
                    })
                    .await;
                let _ = tx_clone.send(AppEvent::ChainCreated(chain)).await;
            }
            Err(err) => {
                let _ = tx_clone
                    .send(AppEvent::AutomationLog(format!(
                        "Chain launch failed: {err}"
                    )))
                    .await;
                let _ = tx_clone
                    .send(AppEvent::CommandResult {
                        label: "chain".to_string(),
                        action: "launch".to_string(),
                        success: false,
                        message: Some(format!("Automation launch failed: {err}")),
                    })
                    .await;
            }
        }
    });
    app.set_status_message(MessageKind::Info, "Automation: launching RBFT testnet...");
}

async fn run_lifecycle_action<F>(
    ctor: F,
    app: &mut AppState,
    tx: mpsc::Sender<AppEvent>,
    label: &str,
) -> Result<()>
where
    F: Fn(usize) -> AppAction,
{
    let idx = app
        .index_for_label(label)
        .ok_or_else(|| anyhow!("Validator {} not found", label))?;
    let action = ctor(idx);
    app.begin_action(&action);
    match dispatch_action(action, app, tx.clone()).await {
        Ok(message) => app.set_status_message(MessageKind::Info, message),
        Err(err) => {
            app.clear_action_state_index(idx);
            app.set_status_message(
                MessageKind::Error,
                format!("Automation lifecycle error for {}: {}", label, err),
            );
        }
    }
    Ok(())
}

async fn launch_additional_validator(
    auto_start: bool,
    app: &mut AppState,
    tx: mpsc::Sender<AppEvent>,
    shared_targets: Arc<RwLock<Vec<ValidatorTarget>>>,
) -> Result<()> {
    let plan = app.plan_launch_node()?;
    {
        let mut targets = shared_targets.write().await;
        targets.push(plan.target.clone());
    }
    let snapshot = {
        let targets = shared_targets.read().await;
        targets.clone()
    };
    let _ = tx.send(AppEvent::TargetUpdate(snapshot)).await;

    if auto_start {
        let action = AppAction::Start(plan.entry_index);
        app.begin_action(&action);
        match dispatch_action(action, app, tx.clone()).await {
            Ok(message) => app.set_status_message(MessageKind::Info, message),
            Err(err) => {
                app.clear_action_state_index(plan.entry_index);
                app.set_status_message(
                    MessageKind::Error,
                    format!("Automation launch node failed: {err}"),
                );
            }
        }
    } else {
        app.set_status_message(
            MessageKind::Info,
            format!(
                "Automation staged validator {} (auto-start disabled)",
                plan.target.label
            ),
        );
    }
    Ok(())
}

async fn spawn_spam_job(
    request: SpamRequest,
    app: &mut AppState,
    tx: mpsc::Sender<AppEvent>,
    shared_targets: Arc<RwLock<Vec<ValidatorTarget>>>,
) -> Result<()> {
    let validators = {
        let targets = shared_targets.read().await;
        targets
            .iter()
            .map(|t| t.url.to_string())
            .collect::<Vec<_>>()
    };

    if validators.is_empty() {
        app.set_status_message(
            MessageKind::Error,
            "Automation spam job skipped, no validators available",
        );
        app.automation_push_log("Spam job skipped: no validators available");
        return Ok(());
    }

    let tx_clone = tx.clone();
    tokio::spawn(async move {
        if let Err(err) = handle_spam_request(request, validators, tx_clone.clone()).await {
            let _ = tx_clone
                .send(AppEvent::AutomationLog(format!("Spam job failed: {err}")))
                .await;
            let _ = tx_clone
                .send(AppEvent::CommandResult {
                    label: "spam".to_string(),
                    action: "spam".to_string(),
                    success: false,
                    message: Some(format!("Automation spam job failed: {err}")),
                })
                .await;
        }
    });
    app.set_status_message(MessageKind::Info, "Automation spam job started");
    app.automation_push_log("Spam job started");
    Ok(())
}
