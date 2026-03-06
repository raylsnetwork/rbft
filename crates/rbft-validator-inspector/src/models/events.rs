// SPDX-License-Identifier: Apache-2.0
use anyhow::{anyhow, Result};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::{Color, Modifier, Style};
use std::path::PathBuf;

use super::state::ActiveChain;
use super::validator::ValidatorTarget;

#[derive(Clone, Copy)]
pub enum MessageKind {
    Info,
    Error,
}

#[derive(Clone, Copy)]
pub enum StatusClass {
    Up,
    Stale,
    Down,
}

impl StatusClass {
    pub fn text(&self) -> &'static str {
        match self {
            StatusClass::Up => "Up",
            StatusClass::Stale => "Stale",
            StatusClass::Down => "Down",
        }
    }

    pub fn style(&self) -> Style {
        match self {
            StatusClass::Up => Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
            StatusClass::Stale => Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
            StatusClass::Down => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum LifecycleState {
    Starting,
    Restarting,
    Stopping,
}

impl LifecycleState {
    pub fn text(&self) -> &'static str {
        match self {
            LifecycleState::Starting => "Starting",
            LifecycleState::Restarting => "Restarting",
            LifecycleState::Stopping => "Stopping",
        }
    }

    pub fn style(&self) -> Style {
        match self {
            LifecycleState::Starting | LifecycleState::Restarting => Style::default()
                .fg(Color::LightBlue)
                .add_modifier(Modifier::BOLD),
            LifecycleState::Stopping => Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        }
    }
}

#[derive(Debug)]
pub enum AppAction {
    Start(usize),
    Stop(usize),
    Restart(usize),
}

impl AppAction {
    pub fn index(&self) -> usize {
        match self {
            AppAction::Start(i) | AppAction::Stop(i) | AppAction::Restart(i) => *i,
        }
    }
}

#[derive(Clone, Debug)]
pub struct AutomationScriptSelection {
    pub relative_path: String,
    pub contract: String,
}

#[derive(Clone, Debug)]
pub struct AutomationRunConfig {
    pub script: AutomationScriptSelection,
}

pub enum KeyOutcome {
    None,
    Action(AppAction),
    LaunchRequest(LaunchRequest),
    LaunchNode(LaunchNodePlan),
    SpamRequest(SpamRequest),
    Ping(usize),
    AutomationRun(AutomationRunConfig),
    AutomationStop,
}

#[derive(Clone, Debug)]
pub enum AutomationAction {
    LaunchChain(LaunchRequest),
    StartValidator(String),
    StopValidator(String),
    RestartValidator(String),
    LaunchValidator { auto_start: bool },
    SpamJob(SpamRequest),
    PingValidator(String),
    Log(String),
}

#[derive(Clone, Debug)]
pub struct LaunchRequest {
    pub count: usize,
    pub base_port: u16,
}

#[derive(Clone, Copy)]
pub enum LaunchField {
    Validators,
    BasePort,
}

impl LaunchField {
    pub fn label(&self) -> &'static str {
        match self {
            LaunchField::Validators => "Validators",
            LaunchField::BasePort => "Base HTTP Port",
        }
    }
}

#[derive(Clone)]
pub struct LaunchForm {
    pub fields: Vec<LaunchField>,
    pub index: usize,
    pub inputs: Vec<String>,
    pub error: Option<String>,
}

impl Default for LaunchForm {
    fn default() -> Self {
        Self::new()
    }
}

impl LaunchForm {
    pub fn new() -> Self {
        Self {
            fields: vec![LaunchField::Validators, LaunchField::BasePort],
            index: 0,
            inputs: vec!["5".into(), "8545".into()],
            error: None,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> LaunchFormOutcome {
        match key.code {
            KeyCode::Esc => return LaunchFormOutcome::Cancelled,
            KeyCode::Tab | KeyCode::Down => {
                self.error = None;
                self.index = (self.index + 1) % self.fields.len();
                return LaunchFormOutcome::Continue;
            }
            KeyCode::Up => {
                self.error = None;
                if self.index == 0 {
                    self.index = self.fields.len() - 1;
                } else {
                    self.index -= 1;
                }
                return LaunchFormOutcome::Continue;
            }
            KeyCode::Backspace => {
                self.error = None;
                if !self.inputs[self.index].is_empty() {
                    self.inputs[self.index].pop();
                }
                return LaunchFormOutcome::Continue;
            }
            KeyCode::Char(c) if key.modifiers.is_empty() => {
                self.error = None;
                self.inputs[self.index].push(c);
                return LaunchFormOutcome::Continue;
            }
            KeyCode::Enter => {
                self.error = None;

                // If we're on the last field, submit
                if self.index == self.fields.len() - 1 {
                    match self.build_request() {
                        Ok(request) => return LaunchFormOutcome::Submitted(request),
                        Err(err) => {
                            self.error = Some(err);
                            return LaunchFormOutcome::Continue;
                        }
                    }
                } else {
                    // Move to next field
                    self.index += 1;
                    return LaunchFormOutcome::Continue;
                }
            }
            _ => {}
        }
        LaunchFormOutcome::Continue
    }

    pub fn validate_field(&mut self, idx: usize) -> Result<(), String> {
        match self.fields[idx] {
            LaunchField::Validators => {
                let value = self.inputs[idx].trim();
                if value.is_empty() {
                    return Err("Validators field cannot be empty.".into());
                }
                let parsed: usize = value.parse().map_err(|e| format!("Invalid number: {e}"))?;
                if parsed == 0 || parsed > 50 {
                    return Err("Validators must be between 1 and 50.".into());
                }
                Ok(())
            }
            LaunchField::BasePort => {
                let value = self.inputs[idx].trim();
                if value.is_empty() {
                    return Err("Base port cannot be empty.".into());
                }

                Ok(())
            }
        }
    }

    pub fn build_request(&self) -> Result<LaunchRequest, String> {
        let count: usize = self.inputs[0]
            .trim()
            .parse()
            .map_err(|e| format!("Invalid validator count: {e}"))?;
        let base_port: u16 = self.inputs[1]
            .trim()
            .parse()
            .map_err(|e| format!("Invalid port: {e}"))?;

        if count == 0 || count > 50 {
            return Err("Validators must be between 1 and 50.".into());
        }

        Ok(LaunchRequest { count, base_port })
    }
}

pub enum LaunchFormOutcome {
    Continue,
    Cancelled,
    Submitted(LaunchRequest),
}

#[derive(Clone)]
pub struct LaunchNodePlan {
    pub entry_index: usize,
    pub target: ValidatorTarget,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpamMode {
    RoundRobin,
    Target,
}

impl SpamMode {
    pub fn label(&self) -> &'static str {
        match self {
            SpamMode::RoundRobin => "round-robin",
            SpamMode::Target => "target",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_lowercase().as_str() {
            "round-robin" | "roundrobin" | "round" => Some(SpamMode::RoundRobin),
            "target" | "targeted" => Some(SpamMode::Target),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SpamRequest {
    pub total_txs: u64,
    pub parallel: u64,
    pub burst: u64,
    pub accounts: usize,
    pub mode: SpamMode,
    pub target_url: Option<String>,
}

#[derive(Clone, Copy)]
pub enum SpamField {
    TotalTxs,
    Parallel,
    Burst,
    Accounts,
    Mode,
    TargetUrl,
}

impl SpamField {
    pub fn label(&self) -> &'static str {
        match self {
            SpamField::TotalTxs => "Total txs",
            SpamField::Parallel => "Parallel txs",
            SpamField::Burst => "Txs per burst",
            SpamField::Accounts => "Accounts",
            SpamField::Mode => "Mode",
            SpamField::TargetUrl => "Target validator URL",
        }
    }
}

#[derive(Clone)]
pub struct SpamForm {
    pub fields: Vec<SpamField>,
    pub index: usize,
    pub inputs: Vec<String>,
    pub error: Option<String>,
}

impl SpamForm {
    pub fn new(default_target: Option<String>) -> Self {
        Self {
            fields: vec![
                SpamField::TotalTxs,
                SpamField::Parallel,
                SpamField::Burst,
                SpamField::Accounts,
                SpamField::Mode,
                SpamField::TargetUrl,
            ],
            index: 0,
            inputs: vec![
                "100".into(),
                "4".into(),
                "20".into(),
                "10".into(),
                "round-robin".into(),
                default_target.unwrap_or_default(),
            ],
            error: None,
        }
    }

    pub fn mode(&self) -> SpamMode {
        SpamMode::parse(&self.inputs[4]).unwrap_or(SpamMode::RoundRobin)
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> SpamFormOutcome {
        match key.code {
            KeyCode::Esc => return SpamFormOutcome::Cancelled,
            KeyCode::Tab | KeyCode::Down => {
                self.error = None;
                self.index = (self.index + 1) % self.fields.len();
                return SpamFormOutcome::Continue;
            }
            KeyCode::Up => {
                self.error = None;
                if self.index == 0 {
                    self.index = self.fields.len() - 1;
                } else {
                    self.index -= 1;
                }
                return SpamFormOutcome::Continue;
            }
            KeyCode::Backspace => {
                self.error = None;
                if !self.inputs[self.index].is_empty() {
                    self.inputs[self.index].pop();
                }
                return SpamFormOutcome::Continue;
            }
            KeyCode::Char(c) if key.modifiers.is_empty() => {
                self.error = None;
                self.inputs[self.index].push(c);
                return SpamFormOutcome::Continue;
            }
            KeyCode::Enter => {
                self.error = None;
                let submitting = if self.index == self.fields.len() - 1 {
                    true
                } else {
                    matches!(self.fields[self.index], SpamField::Mode)
                        && matches!(self.mode(), SpamMode::RoundRobin)
                };
                if submitting {
                    match self.build_request() {
                        Ok(request) => return SpamFormOutcome::Submitted(request),
                        Err(err) => {
                            self.error = Some(err);
                            return SpamFormOutcome::Continue;
                        }
                    }
                } else {
                    self.index += 1;
                    return SpamFormOutcome::Continue;
                }
            }
            _ => {}
        }
        SpamFormOutcome::Continue
    }

    fn build_request(&self) -> Result<SpamRequest, String> {
        let total_txs: u64 = self.inputs[0]
            .trim()
            .parse()
            .map_err(|e| format!("Invalid total txs: {e}"))?;
        if total_txs == 0 {
            return Err("Total transactions must be greater than 0".into());
        }
        let parallel: u64 = self.inputs[1]
            .trim()
            .parse()
            .map_err(|e| format!("Invalid parallel count: {e}"))?;
        if parallel == 0 {
            return Err("Parallel transactions must be at least 1".into());
        }
        let burst: u64 = self.inputs[2]
            .trim()
            .parse()
            .map_err(|e| format!("Invalid burst size: {e}"))?;
        if burst == 0 {
            return Err("Burst size must be at least 1".into());
        }
        let accounts: usize = self.inputs[3]
            .trim()
            .parse()
            .map_err(|e| format!("Invalid account count: {e}"))?;
        if accounts < 2 {
            return Err("Need at least 2 accounts".into());
        }
        let mode = SpamMode::parse(&self.inputs[4])
            .ok_or_else(|| "Mode must be 'round-robin' or 'target'".to_string())?;
        let target = self.inputs[5].trim();
        if matches!(mode, SpamMode::Target) && target.is_empty() {
            return Err("Target validator URL is required for target mode".into());
        }

        Ok(SpamRequest {
            total_txs,
            parallel,
            burst,
            accounts,
            mode,
            target_url: if target.is_empty() {
                None
            } else {
                Some(target.to_string())
            },
        })
    }
}

pub enum SpamFormOutcome {
    Continue,
    Cancelled,
    Submitted(SpamRequest),
}

#[derive(Clone)]
pub struct LogModal {
    pub label: String,
    pub title: String,
    pub path: PathBuf,
    pub lines: Vec<String>,
    pub scroll: u16,
    pub follow_tail: bool,
}

impl LogModal {
    pub fn new(label: String, title: String, path: PathBuf) -> Self {
        Self {
            label,
            title,
            path,
            lines: Vec::new(),
            scroll: 0,
            follow_tail: true,
        }
    }

    pub fn refresh_lines(&mut self) -> Result<()> {
        let content = std::fs::read_to_string(&self.path)
            .map_err(|e| anyhow!("failed to read log file {}: {e}", self.path.display()))?;
        const MAX_LINES: usize = 500;
        let mut lines: Vec<String> = content.lines().map(Self::sanitize_line).collect();
        if lines.len() > MAX_LINES {
            lines = lines[lines.len() - MAX_LINES..].to_vec();
        }
        self.lines = lines;
        Ok(())
    }

    pub fn scroll_up(&mut self, lines: u16) {
        self.follow_tail = false;
        self.scroll = self.scroll.saturating_sub(lines);
    }

    pub fn scroll_down(&mut self, lines: u16) {
        self.follow_tail = false;
        self.scroll = self.scroll.saturating_add(lines);
    }

    pub fn scroll_to_top(&mut self) {
        self.follow_tail = false;
        self.scroll = 0;
    }

    pub fn scroll_to_bottom(&mut self) {
        self.follow_tail = true;
        self.scroll = u16::MAX;
    }

    pub fn sanitize_line(line: &str) -> String {
        enum State {
            Text,
            Ansi,
        }

        let mut state = State::Text;
        let mut cleaned = String::with_capacity(line.len());

        for ch in line.chars() {
            match state {
                State::Text => {
                    if ch == '\x1b' {
                        state = State::Ansi;
                        continue;
                    }
                    if ch == '\r' {
                        continue;
                    }
                    if ch.is_control() && ch != '\t' {
                        continue;
                    }
                    cleaned.push(ch);
                }
                State::Ansi => {
                    if ch.is_ascii_alphabetic() || ch == '~' {
                        state = State::Text;
                    }
                }
            }
        }

        cleaned
    }
}

pub enum AppEvent {
    Input(KeyEvent),
    Tick,
    Data(Vec<super::validator::ValidatorReport>),
    CommandResult {
        label: String,
        action: String,
        success: bool,
        message: Option<String>,
    },
    ChainCreated(ActiveChain),
    TargetUpdate(Vec<super::validator::ValidatorTarget>),
    Automation(AutomationAction),
    AutomationStarted(AutomationScriptSelection),
    AutomationLog(String),
    AutomationStopped {
        success: bool,
        message: String,
    },
}
