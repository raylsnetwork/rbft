// SPDX-License-Identifier: Apache-2.0
pub mod environment;
pub mod paths;

pub use environment::*;
pub use paths::*;

#[derive(Clone)]
pub struct ControlConfig {
    pub start_cmd: Option<String>,
    pub stop_cmd: Option<String>,
    pub restart_cmd: Option<String>,
}

impl ControlConfig {
    pub fn can_start(&self) -> bool {
        self.start_cmd.is_some()
    }

    pub fn can_stop(&self) -> bool {
        self.stop_cmd.is_some()
    }

    pub fn can_restart(&self) -> bool {
        self.restart_cmd.is_some() || (self.can_start() && self.can_stop())
    }
}
