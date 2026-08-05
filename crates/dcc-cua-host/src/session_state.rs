use std::collections::HashMap;

use dcc_cua_browser::BrowserSession;
use dcc_cua_core::{ComputerUseDesktopSession, ComputerUseSession};
use dcc_cua_shm::SharedImage;
use serde_json::Value;

pub(super) struct HostSession {
    pub(super) runtime_session_id: String,
    pub(super) task_grant_id: String,
    pub(super) allow_raw_input: bool,
    pub(super) allow_app_terminate: bool,
    pub(super) allow_clipboard_read: bool,
    pub(super) allow_clipboard_write: bool,
    pub(super) allow_recording: bool,
    pub(super) allow_browser_input: bool,
    pub(super) allow_browser_prepare: bool,
    pub(super) allow_browser_download: bool,
    pub(super) allow_native_tool: bool,
    pub(super) allow_menu_invoke: bool,
    pub(super) allow_session_escalation: bool,
    pub(super) capability: String,
    pub(super) session: ComputerUseSession,
    pub(super) browser: BrowserSession,
    pub(super) latest_observation_id: Option<String>,
    pub(super) latest_accessibility_state_id: Option<String>,
    pub(super) latest_accessibility_root: Option<Value>,
    pub(super) latest_shared_image: Option<SharedImage>,
}

impl HostSession {
    pub(super) fn invalidate_observations(&mut self) {
        self.latest_observation_id = None;
        self.latest_accessibility_state_id = None;
        self.latest_accessibility_root = None;
        self.latest_shared_image = None;
        self.browser.invalidate_snapshot();
    }
}

pub(super) struct HostDesktopSession {
    pub(super) runtime_session_id: String,
    pub(super) task_grant_id: String,
    pub(super) allow_raw_input: bool,
    pub(super) capability: String,
    pub(super) interrupt_generation: u64,
    pub(super) session: ComputerUseDesktopSession,
    pub(super) latest_shared_image: Option<SharedImage>,
}

#[derive(Clone)]
pub(super) struct HostLaunchSession {
    pub(super) runtime_session_id: String,
    pub(super) task_grant_id: String,
    pub(super) application_label: String,
    pub(super) process_id: u32,
}

#[derive(Default)]
pub(super) struct ConnectionSessions {
    pub(super) agent_name: String,
    pub(super) windows: HashMap<String, HostSession>,
    pub(super) desktops: HashMap<String, HostDesktopSession>,
    pub(super) launches: HashMap<String, HostLaunchSession>,
}
