// SPDX-License-Identifier: Apache-2.0
// Module declarations
mod automation;
mod cli;
mod commands;
mod config;
mod models;
mod rpc;
mod ui;
mod utils;

// Re-exports
pub use cli::Cli;
pub use commands::{dispatch_action, handle_launch_chain};
pub use config::{
    default_test_dirs, load_trusted_peers, load_validator_key, preload_environment, ControlConfig,
};
pub use models::{
    AppAction, AppEvent, AppState, KeyOutcome, LaunchForm, LaunchNodePlan, LaunchRequest, LogModal,
    ValidatorEntry, ValidatorTarget,
};
pub use rpc::{fetch_all, parse_targets, spawn_fetcher};
pub use ui::{cleanup_terminal, run_app, spawn_input, spawn_ticks};

// External imports
use anyhow::Result;
use clap::Parser;
use crossterm::execute;
use crossterm::terminal::{enable_raw_mode, EnterAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};

#[tokio::main]
async fn main() -> Result<()> {
    preload_environment();
    let cli = Cli::parse();
    let initial_targets = parse_targets(&cli)?;
    let refresh = Duration::from_secs(cli.refresh_secs.max(1));
    let control = ControlConfig {
        start_cmd: cli.start_cmd.clone(),
        stop_cmd: cli.stop_cmd.clone(),
        restart_cmd: cli.restart_cmd.clone(),
    };
    let automation_project = cli.automation_project.clone().or_else(|| {
        let fallback = default_automation_project();
        if fallback.exists() {
            Some(fallback)
        } else {
            None
        }
    });

    let shared_targets = Arc::new(RwLock::new(initial_targets.clone()));
    let (tx, mut rx) = mpsc::channel::<AppEvent>(64);

    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    // Spawn background tasks
    tokio::spawn(spawn_input(tx.clone()));
    tokio::spawn(spawn_ticks(tx.clone()));
    tokio::spawn(spawn_fetcher(tx.clone(), shared_targets.clone(), refresh));

    let mut app = AppState::new(
        initial_targets
            .into_iter()
            .map(ValidatorEntry::new)
            .collect(),
        control,
        automation_project,
        cli.rbft_bin_dir.clone(),
    );

    let res = run_app(
        &mut terminal,
        &mut app,
        &mut rx,
        tx.clone(),
        shared_targets.clone(),
    )
    .await;

    cleanup_terminal(&mut terminal)?;

    if let Err(err) = res {
        eprintln!("Application error: {}", err);
        std::process::exit(1);
    }

    Ok(())
}

fn default_automation_project() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("automation")
}
