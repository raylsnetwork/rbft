// SPDX-License-Identifier: Apache-2.0
use anyhow::{anyhow, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use url::Url;

use super::events::{
    AppAction, AutomationRunConfig, AutomationScriptSelection, KeyOutcome, LaunchForm,
    LaunchFormOutcome, LaunchNodePlan, LifecycleState, LogModal, MessageKind, SpamForm,
    SpamFormOutcome, StatusClass,
};
use super::validator::{ValidatorEntry, ValidatorReport, ValidatorTarget};
use crate::config::{load_validator_key, ControlConfig};

#[derive(Clone)]
pub struct ActiveChain {
    pub base_http_port: u16,
    pub num_nodes: usize,
    pub data_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub assets_dir: PathBuf,
    pub rbft_binary: PathBuf,
    pub trusted_peers: Option<String>,
}

pub struct AppState {
    pub entries: Vec<ValidatorEntry>,
    pub table_state: TableState,
    pub should_quit: bool,
    pub status_message: Option<(MessageKind, String)>,
    status_from_data: bool,
    pub control: ControlConfig,
    pub rbft_bin_dir: PathBuf,
    pub launch_form: Option<LaunchForm>,
    pub spam_form: Option<SpamForm>,
    pub log_modal: Option<LogModal>,
    pub active_chain: Option<ActiveChain>,
    pub inline_log: InlineLogView,
    pub inline_last_refresh: Option<Instant>,
    pub inline_logs_enabled: bool,
    pub start_time: Option<Instant>,
    pub receipts_scroll: u16,
    pub automation_project: Option<PathBuf>,
    pub automation_scripts: Vec<AutomationScript>,
    pub automation_modal: Option<AutomationModal>,
    pub automation_panel: AutomationPanel,
}

#[derive(Default)]
pub struct InlineLogView {
    pub label: Option<String>,
    pub lines: Vec<String>,
    pub error: Option<String>,
}

pub struct ChainStats {
    pub uptime: Option<Duration>,
    pub total_validators: usize,
    pub active_validators: usize,
    pub total_txs: u64,
    pub highest_block: Option<u64>,
    pub average_latency: Option<Duration>,
}

#[derive(Clone)]
pub struct ReceiptView {
    pub label: String,
    pub tx_hash: String,
    pub block_number: Option<u64>,
    pub status: Option<bool>,
    pub gas_used: Option<u64>,
    pub to: Option<String>,
    pub from: Option<String>,
}

#[derive(Clone)]
pub struct AutomationScript {
    pub relative_path: String,
    pub contract: String,
}

impl AutomationScript {
    pub fn display_name(&self) -> String {
        format!("{} ({})", self.contract, self.relative_path)
    }
}

#[derive(Clone)]
pub struct AutomationModal {
    pub scripts: Vec<AutomationScript>,
    pub index: usize,
}

impl AutomationModal {
    fn new(scripts: Vec<AutomationScript>) -> Self {
        Self { scripts, index: 0 }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> AutomationModalOutcome {
        let count = self.scripts.len();
        match key.code {
            KeyCode::Esc => AutomationModalOutcome::Cancelled,
            KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => {
                if count == 0 {
                    return AutomationModalOutcome::Continue;
                }
                if self.index == 0 {
                    self.index = count - 1;
                } else {
                    self.index -= 1;
                }
                AutomationModalOutcome::Continue
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => {
                if count == 0 {
                    return AutomationModalOutcome::Continue;
                }
                self.index = (self.index + 1) % count;
                AutomationModalOutcome::Continue
            }
            KeyCode::Home => {
                if count == 0 {
                    return AutomationModalOutcome::Continue;
                }
                self.index = 0;
                AutomationModalOutcome::Continue
            }
            KeyCode::End => {
                if count == 0 {
                    return AutomationModalOutcome::Continue;
                }
                self.index = count - 1;
                AutomationModalOutcome::Continue
            }
            KeyCode::Enter => {
                if count == 0 {
                    AutomationModalOutcome::Cancelled
                } else {
                    AutomationModalOutcome::Selected(AutomationScriptSelection {
                        relative_path: self.scripts[self.index].relative_path.clone(),
                        contract: self.scripts[self.index].contract.clone(),
                    })
                }
            }
            _ => AutomationModalOutcome::Continue,
        }
    }
}

pub enum AutomationModalOutcome {
    Continue,
    Cancelled,
    Selected(AutomationScriptSelection),
}

pub struct AutomationPanel {
    pub running: bool,
    pub script_label: Option<String>,
    pub lines: Vec<String>,
}

impl Default for AutomationPanel {
    fn default() -> Self {
        Self {
            running: false,
            script_label: None,
            lines: vec!["Automation idle".into()],
        }
    }
}

impl AutomationPanel {
    pub fn start(&mut self, script_label: impl Into<String>) {
        self.running = true;
        self.script_label = Some(script_label.into());
        self.lines.clear();
        self.lines.push("Starting automation script...".into());
    }

    pub fn stop(&mut self, reason: impl Into<String>) {
        self.running = false;
        self.script_label = None;
        self.lines.push(reason.into());
        if self.lines.is_empty() {
            self.lines.push("Automation idle".into());
        }
    }

    pub fn push_log(&mut self, line: impl Into<String>) {
        const MAX_LINES: usize = 200;
        self.lines.push(line.into());
        if self.lines.len() > MAX_LINES {
            self.lines.drain(0..self.lines.len() - MAX_LINES);
        }
    }
}

fn discover_automation_scripts(base: Option<&PathBuf>) -> Vec<AutomationScript> {
    let Some(root) = base else {
        return Vec::new();
    };
    let scripts_dir = root.join("contracts").join("scripts");
    let mut entries = Vec::new();
    if let Ok(read_dir) = fs::read_dir(&scripts_dir) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("sol") {
                continue;
            }
            let contract = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            let relative_path = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            entries.push(AutomationScript {
                relative_path,
                contract,
            });
        }
    }
    entries.sort_by_key(|entry| entry.display_name());
    entries
}

fn tail_log(path: &Path) -> Result<Vec<String>> {
    const MAX_BYTES: u64 = 16 * 1024; // 16 KiB tail to avoid loading huge logs
    const MAX_LINES: usize = 100;
    let mut file = std::fs::File::open(path)?;
    let metadata = file.metadata()?;
    let len = metadata.len();
    let start = len.saturating_sub(MAX_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut buf = String::new();
    file.read_to_string(&mut buf)?;
    let mut lines: Vec<String> = buf.lines().map(LogModal::sanitize_line).collect();
    if lines.len() > MAX_LINES {
        lines = lines[lines.len() - MAX_LINES..].to_vec();
    }
    Ok(lines)
}

impl InlineLogView {
    pub fn clear(&mut self) {
        self.label = None;
        self.lines.clear();
        self.error = None;
    }

    pub fn set_lines(&mut self, label: String, lines: Vec<String>) {
        self.label = Some(label);
        self.lines = lines;
        self.error = None;
    }

    pub fn set_error(&mut self, label: Option<String>, message: impl Into<String>) {
        self.label = label;
        self.lines.clear();
        self.error = Some(message.into());
    }
}

impl AppState {
    pub fn new(
        entries: Vec<ValidatorEntry>,
        control: ControlConfig,
        automation_project: Option<PathBuf>,
        rbft_bin_dir: PathBuf,
    ) -> Self {
        let mut table_state = TableState::default();
        if !entries.is_empty() {
            table_state.select(Some(0));
        }
        let automation_scripts = discover_automation_scripts(automation_project.as_ref());
        let mut state = Self {
            entries,
            table_state,
            should_quit: false,
            status_message: None,
            status_from_data: false,
            control,
            rbft_bin_dir,
            launch_form: None,
            spam_form: None,
            log_modal: None,
            active_chain: None,
            inline_log: InlineLogView::default(),
            inline_last_refresh: None,
            inline_logs_enabled: true,
            start_time: None,
            receipts_scroll: 0,
            automation_project,
            automation_scripts,
            automation_modal: None,
            automation_panel: AutomationPanel::default(),
        };
        state.refresh_inline_log();
        state
    }

    pub fn set_status_message(&mut self, kind: MessageKind, msg: impl Into<String>) {
        self.status_message = Some((kind, msg.into()));
        self.status_from_data = false;
    }

    pub fn set_data_status_message(&mut self, kind: MessageKind, msg: impl Into<String>) {
        self.status_message = Some((kind, msg.into()));
        self.status_from_data = true;
    }

    pub fn clear_error_message(&mut self) {
        if self.status_from_data && matches!(self.status_message, Some((MessageKind::Error, _))) {
            self.status_message = None;
            self.status_from_data = false;
        }
    }

    pub fn set_active_chain(&mut self, chain: ActiveChain) {
        self.active_chain = Some(chain);
        if self.start_time.is_none() {
            self.start_time = Some(Instant::now());
        }
        self.refresh_inline_log();
    }

    pub fn plan_launch_node(&mut self) -> Result<LaunchNodePlan> {
        let chain = self
            .active_chain
            .as_mut()
            .ok_or_else(|| anyhow!("Launch node is only available after launching a chain"))?;

        let next_index = self
            .entries
            .iter()
            .filter_map(|entry| {
                entry
                    .target
                    .label
                    .trim_start_matches('v')
                    .parse::<usize>()
                    .ok()
            })
            .max()
            .map(|max_idx| max_idx + 1)
            .unwrap_or(0);

        // Ensure the chain metadata covers the new validator
        chain.num_nodes = chain.num_nodes.max(next_index + 1);

        let target = chain.target_for_index(next_index)?;
        let entry_index = self.entries.len();
        let mut entry = ValidatorEntry::new(target.clone());
        entry.set_lifecycle(Some(LifecycleState::Starting));
        self.entries.push(entry);
        self.table_state.select(Some(entry_index));
        self.receipts_scroll = 0;
        self.refresh_inline_log();

        Ok(LaunchNodePlan {
            entry_index,
            target,
        })
    }

    pub fn open_logs_modal(&mut self, idx: usize) -> Result<()> {
        let entry = self
            .entries
            .get(idx)
            .ok_or_else(|| anyhow!("No validator selected"))?;
        let path = entry
            .target
            .log_path
            .clone()
            .ok_or_else(|| anyhow!("Log path unavailable for validator"))?;
        let label = entry.target.label.clone();
        let mut modal = LogModal::new(label, format!("Logs for {}", entry.target.label), path);
        modal.refresh_lines()?;
        self.log_modal = Some(modal);
        Ok(())
    }

    pub fn close_log_modal(&mut self) {
        self.log_modal = None;
    }

    pub fn refresh_log_for_label(&mut self, label: &str) -> Result<()> {
        if let Some(modal) = self.log_modal.as_mut() {
            if modal.label == label {
                modal.refresh_lines()?;
                if modal.follow_tail {
                    modal.scroll_to_bottom();
                }
            }
        }
        Ok(())
    }

    pub fn refresh_log_for_index(&mut self, idx: usize) -> Result<()> {
        if let Some(label) = self
            .entries
            .get(idx)
            .map(|entry| entry.target.label.clone())
        {
            self.refresh_log_for_label(&label)?;
        }
        Ok(())
    }

    pub fn reset_log_file_for_index(&mut self, idx: usize) {
        let (path, label) = match self.entries.get(idx) {
            Some(entry) => match entry.target.log_path.clone() {
                Some(path) => (path, entry.target.label.clone()),
                None => return,
            },
            None => return,
        };
        if let Err(err) = std::fs::write(&path, "") {
            self.set_status_message(
                MessageKind::Error,
                format!("Failed to clear log {}: {err}", path.display()),
            );
            return;
        }
        let _ = self.refresh_log_for_label(&label);
    }

    pub fn refresh_logs_after_action(&mut self, label: &str, action: &str) {
        if matches!(
            action,
            "start" | "restart" | "start-ready" | "restart-ready"
        ) {
            if let Err(err) = self.refresh_log_for_label(label) {
                self.set_status_message(
                    MessageKind::Error,
                    format!("Failed to refresh logs: {err}"),
                );
            }
            if self
                .inline_log
                .label
                .as_deref()
                .map(|current| current == label)
                .unwrap_or(false)
            {
                self.refresh_inline_log();
            }
        }
    }

    pub fn begin_action(&mut self, action: &AppAction) {
        let idx = action.index();
        if let Some(entry) = self.entries.get_mut(idx) {
            let lifecycle = match action {
                AppAction::Start(_) => Some(LifecycleState::Starting),
                AppAction::Stop(_) => Some(LifecycleState::Stopping),
                AppAction::Restart(_) => Some(LifecycleState::Restarting),
            };
            entry.set_lifecycle(lifecycle);
        }
        if matches!(action, AppAction::Start(_) | AppAction::Restart(_)) {
            self.reset_log_file_for_index(idx);
            if let Err(err) = self.refresh_log_for_index(idx) {
                self.set_status_message(
                    MessageKind::Error,
                    format!("Failed to refresh logs: {err}"),
                );
            }
        }
    }

    pub fn clear_action_state_index(&mut self, idx: usize) {
        if let Some(entry) = self.entries.get_mut(idx) {
            entry.set_lifecycle(None);
        }
    }

    pub fn clear_action_state_label(&mut self, label: &str) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.target.label == label) {
            entry.set_lifecycle(None);
        }
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.table_state.selected()
    }

    pub fn selected_entry(&self) -> Option<&ValidatorEntry> {
        self.selected_index().and_then(|idx| self.entries.get(idx))
    }

    pub fn selected_entry_mut(&mut self) -> Option<&mut ValidatorEntry> {
        let idx = self.selected_index()?;
        self.entries.get_mut(idx)
    }

    pub fn index_for_label(&self, label: &str) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.target.label == label)
    }

    pub fn receipt_entries(&self) -> Vec<ReceiptView> {
        if let Some(entry) = self.selected_entry() {
            entry
                .receipts
                .iter()
                .rev()
                .map(|receipt| ReceiptView {
                    label: entry.target.label.clone(),
                    tx_hash: receipt.tx_hash.clone(),
                    block_number: receipt.block_number,
                    status: receipt.status,
                    gas_used: receipt.gas_used,
                    to: receipt.to.clone(),
                    from: receipt.from.clone(),
                })
                .collect()
        } else {
            Vec::new()
        }
    }

    pub fn automation_script_label(&self) -> Option<&str> {
        self.automation_panel.script_label.as_deref()
    }

    pub fn automation_log_lines(&self) -> &[String] {
        &self.automation_panel.lines
    }

    pub fn automation_is_running(&self) -> bool {
        self.automation_panel.running
    }

    pub fn automation_scripts_available(&self) -> bool {
        !self.automation_scripts.is_empty()
    }

    pub fn automation_project_label(&self) -> Option<String> {
        self.automation_project
            .as_ref()
            .map(|path| path.display().to_string())
    }

    pub fn automation_start(&mut self, script_label: impl Into<String>) {
        self.automation_panel.start(script_label);
    }

    pub fn automation_push_log(&mut self, line: impl Into<String>) {
        self.automation_panel.push_log(line);
    }

    pub fn automation_stop(&mut self, reason: impl Into<String>) {
        self.automation_panel.stop(reason);
    }

    fn receipt_len(&self) -> usize {
        self.selected_entry()
            .map(|entry| entry.receipts.len())
            .unwrap_or(0)
    }

    pub fn scroll_receipts_up(&mut self, amount: u16) {
        self.receipts_scroll = self.receipts_scroll.saturating_sub(amount);
    }

    pub fn scroll_receipts_down(&mut self, amount: u16) {
        let len = self.receipt_len();
        if len == 0 {
            return;
        }
        let max_scroll = len.saturating_sub(1).min(u16::MAX as usize) as u16;
        self.receipts_scroll = (self.receipts_scroll.saturating_add(amount)).min(max_scroll);
    }

    fn clamp_receipt_scroll(&mut self) {
        let len = self.receipt_len();
        if len == 0 {
            self.receipts_scroll = 0;
            return;
        }
        let max_scroll = len.saturating_sub(1).min(u16::MAX as usize) as u16;
        if self.receipts_scroll > max_scroll {
            self.receipts_scroll = max_scroll;
        }
    }

    pub fn inline_log_label(&self) -> Option<&str> {
        self.inline_log.label.as_deref()
    }

    pub fn inline_log_lines(&self) -> &[String] {
        &self.inline_log.lines
    }

    pub fn inline_logs_enabled(&self) -> bool {
        self.inline_logs_enabled
    }

    pub fn inline_log_error(&self) -> Option<&str> {
        self.inline_log.error.as_deref()
    }

    pub fn refresh_inline_log(&mut self) {
        if !self.inline_logs_enabled {
            return;
        }
        let now = Instant::now();
        if let Some(last) = self.inline_last_refresh {
            if now.duration_since(last) < Duration::from_secs(2) {
                return;
            }
        }

        let idx = match self.table_state.selected() {
            Some(idx) => idx,
            None => {
                self.inline_log.clear();
                return;
            }
        };
        let entry = match self.entries.get(idx) {
            Some(entry) => entry,
            None => {
                self.inline_log.clear();
                return;
            }
        };

        match entry.target.log_path.as_ref() {
            Some(path) => match tail_log(path) {
                Ok(lines) => {
                    if lines.is_empty() {
                        self.inline_log
                            .set_lines(entry.target.label.clone(), vec!["<log empty>".into()]);
                    } else {
                        self.inline_log.set_lines(entry.target.label.clone(), lines);
                    }
                }
                Err(err) => self.inline_log.set_error(
                    Some(entry.target.label.clone()),
                    format!("Unable to read log: {err}"),
                ),
            },
            None => self
                .inline_log
                .set_error(Some(entry.target.label.clone()), "No log file configured"),
        }
        self.inline_last_refresh = Some(now);
    }

    pub fn chain_stats(&self) -> ChainStats {
        let total_validators = self.entries.len();
        let now = Instant::now();
        let active_validators = self
            .entries
            .iter()
            .filter(|entry| matches!(entry.status_class(now), StatusClass::Up))
            .count();
        let total_txs = self.entries.iter().map(|entry| entry.total_txs).sum();
        let highest_block = self.entries.iter().filter_map(|e| e.height).max();

        let mut latency_sum = Duration::default();
        let mut latency_count = 0u32;
        for entry in &self.entries {
            if let Some(latency) = entry.latency {
                latency_sum += latency;
                latency_count += 1;
            }
        }
        let average_latency = if latency_count > 0 {
            Some(latency_sum / latency_count)
        } else {
            None
        };

        let uptime = self
            .start_time
            .map(|start| now.saturating_duration_since(start));

        ChainStats {
            uptime,
            total_validators,
            active_validators,
            total_txs,
            highest_block,
            average_latency,
        }
    }

    pub fn set_targets(&mut self, targets: Vec<ValidatorTarget>) {
        let mut existing: HashMap<String, ValidatorEntry> = self
            .entries
            .drain(..)
            .map(|entry| (entry.target.label.clone(), entry))
            .collect();

        let mut new_entries = Vec::with_capacity(targets.len());
        for target in targets {
            if let Some(mut entry) = existing.remove(&target.label) {
                entry.target = target;
                new_entries.push(entry);
            } else {
                new_entries.push(ValidatorEntry::new(target));
            }
        }

        self.entries = new_entries;
        match self.table_state.selected() {
            Some(idx) if idx < self.entries.len() => {}
            _ => {
                if self.entries.is_empty() {
                    self.table_state.select(None);
                } else {
                    self.table_state.select(Some(0));
                }
            }
        }
        self.receipts_scroll = 0;
        self.refresh_inline_log();
    }

    pub fn open_automation_modal(&mut self) {
        if self.automation_scripts.is_empty() {
            self.set_status_message(
                MessageKind::Error,
                "No automation scripts found under contracts/scripts",
            );
        }
        self.automation_modal = Some(AutomationModal::new(self.automation_scripts.clone()));
    }

    pub fn clear_automation_modal(&mut self) {
        self.automation_modal = None;
    }

    pub fn automation_panel(&self) -> &AutomationPanel {
        &self.automation_panel
    }

    pub fn automation_panel_mut(&mut self) -> &mut AutomationPanel {
        &mut self.automation_panel
    }
    pub fn on_key(&mut self, key: KeyEvent) -> KeyOutcome {
        if let Some(modal) = &mut self.log_modal {
            if matches!(key.code, KeyCode::Esc) {
                self.close_log_modal();
                return KeyOutcome::None;
            }
            let mut refresh_error: Option<String> = None;
            if matches!(key.code, KeyCode::Down | KeyCode::PageDown | KeyCode::End) {
                if let Err(err) = modal.refresh_lines() {
                    refresh_error = Some(err.to_string());
                }
            }
            match key.code {
                KeyCode::Up => modal.scroll_up(1),
                KeyCode::Down => modal.scroll_down(1),
                KeyCode::PageUp => modal.scroll_up(10),
                KeyCode::PageDown => modal.scroll_down(10),
                KeyCode::Home => modal.scroll_to_top(),
                KeyCode::End => modal.scroll_to_bottom(),
                _ => {}
            }
            if let Some(err) = refresh_error {
                self.set_status_message(
                    MessageKind::Error,
                    format!("Failed to refresh logs: {err}"),
                );
            }
            return KeyOutcome::None;
        }
        if let Some(form) = &mut self.launch_form {
            return match form.handle_key(key) {
                LaunchFormOutcome::Continue => KeyOutcome::None,
                LaunchFormOutcome::Cancelled => {
                    self.launch_form = None;
                    KeyOutcome::None
                }
                LaunchFormOutcome::Submitted(request) => {
                    self.launch_form = None;
                    KeyOutcome::LaunchRequest(request)
                }
            };
        }
        if let Some(form) = &mut self.spam_form {
            return match form.handle_key(key) {
                SpamFormOutcome::Continue => KeyOutcome::None,
                SpamFormOutcome::Cancelled => {
                    self.spam_form = None;
                    KeyOutcome::None
                }
                SpamFormOutcome::Submitted(request) => {
                    self.spam_form = None;
                    KeyOutcome::SpamRequest(request)
                }
            };
        }
        if let Some(modal) = &mut self.automation_modal {
            return match modal.handle_key(key) {
                AutomationModalOutcome::Continue => KeyOutcome::None,
                AutomationModalOutcome::Cancelled => {
                    self.clear_automation_modal();
                    KeyOutcome::None
                }
                AutomationModalOutcome::Selected(selection) => {
                    self.clear_automation_modal();
                    KeyOutcome::AutomationRun(AutomationRunConfig { script: selection })
                }
            };
        }

        let now = Instant::now();
        match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') => {
                self.should_quit = true;
                KeyOutcome::None
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
                KeyOutcome::None
            }
            KeyCode::Esc => {
                self.should_quit = true;
                KeyOutcome::None
            }
            KeyCode::Up => {
                self.select_previous(1);
                KeyOutcome::None
            }
            KeyCode::Down => {
                self.select_next(1);
                KeyOutcome::None
            }
            KeyCode::Char('[') => {
                self.scroll_receipts_up(1);
                KeyOutcome::None
            }
            KeyCode::Char(']') => {
                self.scroll_receipts_down(1);
                KeyOutcome::None
            }
            KeyCode::PageUp => {
                self.select_previous(5);
                KeyOutcome::None
            }
            KeyCode::PageDown => {
                self.select_next(5);
                KeyOutcome::None
            }
            KeyCode::Home => {
                self.select_first();
                KeyOutcome::None
            }
            KeyCode::End => {
                self.select_last();
                KeyOutcome::None
            }
            KeyCode::Enter => {
                if let Some(idx) = self.selected_index() {
                    if let Err(err) = self.open_logs_modal(idx) {
                        self.set_status_message(MessageKind::Error, err.to_string());
                    }
                }
                KeyOutcome::None
            }
            KeyCode::Char('p') | KeyCode::Char('P') => {
                let idx = match self.selected_index() {
                    Some(i) => i,
                    None => {
                        self.set_status_message(MessageKind::Info, "No validator selected");
                        return KeyOutcome::None;
                    }
                };
                KeyOutcome::Ping(idx)
            }
            KeyCode::Char('l') | KeyCode::Char('L') => {
                self.launch_form = Some(LaunchForm::new());
                KeyOutcome::None
            }
            KeyCode::Char('t') | KeyCode::Char('T') => {
                let default_target = self
                    .selected_entry()
                    .map(|entry| entry.target.url.to_string());
                self.spam_form = Some(SpamForm::new(default_target));
                KeyOutcome::None
            }
            KeyCode::Char('m') | KeyCode::Char('M') => match self.plan_launch_node() {
                Ok(plan) => KeyOutcome::LaunchNode(plan),
                Err(err) => {
                    self.set_status_message(MessageKind::Error, err.to_string());
                    KeyOutcome::None
                }
            },
            KeyCode::Char('a') | KeyCode::Char('A') => {
                self.open_automation_modal();
                KeyOutcome::None
            }
            KeyCode::Char('x') | KeyCode::Char('X') => {
                if self.automation_is_running() {
                    KeyOutcome::AutomationStop
                } else {
                    KeyOutcome::None
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                let idx = match self.selected_index() {
                    Some(i) => i,
                    None => {
                        self.set_status_message(MessageKind::Info, "No validator selected");
                        return KeyOutcome::None;
                    }
                };
                if !self.control.can_start() && self.active_chain.is_none() {
                    self.set_status_message(
                        MessageKind::Error,
                        "Start command not configured and no launch metadata available",
                    );
                    return KeyOutcome::None;
                }
                let status = self.entries[idx].status_class(now);
                if matches!(status, StatusClass::Up) {
                    self.set_status_message(
                        MessageKind::Info,
                        "Validator already running; Kill first to relaunch",
                    );
                    return KeyOutcome::None;
                }
                KeyOutcome::Action(AppAction::Start(idx))
            }
            KeyCode::Char('k') | KeyCode::Char('K') => {
                let idx = match self.selected_index() {
                    Some(i) => i,
                    None => {
                        self.set_status_message(MessageKind::Info, "No validator selected");
                        return KeyOutcome::None;
                    }
                };
                if !self.control.can_stop() && self.entries[idx].target.http_port().is_none() {
                    self.set_status_message(
                        MessageKind::Error,
                        "Kill command not configured and validator port unknown",
                    );
                    return KeyOutcome::None;
                }
                let status = self.entries[idx].status_class(now);
                if matches!(status, StatusClass::Down) {
                    self.set_status_message(MessageKind::Info, "Validator already stopped");
                    return KeyOutcome::None;
                }
                KeyOutcome::Action(AppAction::Stop(idx))
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                if !self.control.can_restart() {
                    return KeyOutcome::None;
                }
                let idx = match self.selected_index() {
                    Some(i) => i,
                    None => {
                        self.set_status_message(MessageKind::Info, "No validator selected");
                        return KeyOutcome::None;
                    }
                };
                if !self.control.can_restart() {
                    self.set_status_message(
                        MessageKind::Error,
                        "Restart not configured (set VALIDATOR_RESTART_CMD or both START/STOP)",
                    );
                    return KeyOutcome::None;
                }
                let status = self.entries[idx].status_class(now);
                if matches!(status, StatusClass::Down) && self.control.can_start() {
                    KeyOutcome::Action(AppAction::Start(idx))
                } else {
                    KeyOutcome::Action(AppAction::Restart(idx))
                }
            }
            _ => KeyOutcome::None,
        }
    }

    pub fn select_previous(&mut self, count: usize) {
        if self.entries.is_empty() {
            return;
        }
        let current = self.table_state.selected().unwrap_or(0);
        let new = current.saturating_sub(count);
        self.table_state.select(Some(new));
        self.receipts_scroll = 0;
        self.inline_last_refresh = None;
        self.refresh_inline_log();
    }

    pub fn select_next(&mut self, count: usize) {
        if self.entries.is_empty() {
            return;
        }
        let current = self.table_state.selected().unwrap_or(0);
        let max_index = self.entries.len() - 1;
        let new = (current + count).min(max_index);
        self.table_state.select(Some(new));
        self.receipts_scroll = 0;
        self.inline_last_refresh = None;
        self.refresh_inline_log();
    }

    pub fn select_first(&mut self) {
        if !self.entries.is_empty() {
            self.table_state.select(Some(0));
            self.receipts_scroll = 0;
            self.inline_last_refresh = None;
            self.refresh_inline_log();
        }
    }

    pub fn select_last(&mut self) {
        if !self.entries.is_empty() {
            self.table_state.select(Some(self.entries.len() - 1));
            self.receipts_scroll = 0;
            self.inline_last_refresh = None;
            self.refresh_inline_log();
        }
    }

    pub fn apply_reports(&mut self, reports: Vec<ValidatorReport>) {
        let mut map: HashMap<String, ValidatorReport> = HashMap::with_capacity(reports.len());
        for report in reports {
            map.insert(report.label.clone(), report);
        }
        for entry in &mut self.entries {
            if let Some(report) = map.remove(&entry.target.label) {
                entry.apply_report(report);
            }
        }
        self.refresh_inline_log();
        self.clamp_receipt_scroll();
    }

    pub fn handle_command_result(
        &mut self,
        label: &str,
        action: &str,
        success: bool,
        message: Option<String>,
    ) {
        self.refresh_logs_after_action(label, action);
        match action {
            "stop" => self.clear_action_state_label(label),
            "start" | "restart" => {
                if !success {
                    self.clear_action_state_label(label);
                }
            }
            "start-ready" | "restart-ready" => {
                self.clear_action_state_label(label);
            }
            _ => {}
        }
        if success {
            if let Some(entry) = self.entries.iter_mut().find(|e| e.target.label == label) {
                match action {
                    "stop" => {
                        entry.last_ok = None;
                        entry.height = None;
                        entry.total_txs = 0;
                    }
                    "restart" => {
                        entry.last_ok = None;
                        entry.height = None;
                        entry.total_txs = 0;
                    }
                    "start" => {}
                    "start-ready" | "restart-ready" => {
                        entry.last_ok = Some(Instant::now());
                    }
                    _ => {}
                }
            }
        }

        let msg = message.unwrap_or_else(|| {
            if success {
                format!("{action} command succeeded for {label}")
            } else {
                format!("{action} command failed for {label}")
            }
        });
        let kind = if success {
            MessageKind::Info
        } else {
            MessageKind::Error
        };
        self.set_status_message(kind, msg);
    }
}

impl ActiveChain {
    pub fn target_for_index(&self, idx: usize) -> Result<ValidatorTarget> {
        let idx_offset = u16::try_from(idx)
            .map_err(|_| anyhow!("validator index {idx} exceeds supported range"))?;
        let port = self
            .base_http_port
            .checked_sub(idx_offset)
            .ok_or_else(|| anyhow!("base HTTP port too small for validator index {}", idx))?;
        let url = Url::parse(&format!("http://127.0.0.1:{port}"))?;
        Ok(ValidatorTarget {
            label: format!("v{idx}"),
            url,
            port: Some(port),
            key: load_validator_key(&format!("v{idx}")),
            log_path: Some(self.logs_dir.join(format!("node{idx}.log"))),
        })
    }
}
