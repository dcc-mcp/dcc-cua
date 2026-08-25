use serde_json::Value;

use super::HostAction;

impl HostAction {
    pub(super) fn safety_tier(&self, accessibility_root: Option<&Value>) -> HostActionSafetyTier {
        let base_tier = if self.uses_physical_keyboard() {
            if self.input_kind == "raw_input" && self.is_task_granted_keyboard_input() {
                HostActionSafetyTier::TaskGrant
            } else {
                HostActionSafetyTier::ActionConfirmation
            }
        } else if self.input_kind == "semantic" {
            accessibility_root
                .and_then(|root| self.semantic_element(root))
                .and_then(|element| element["policy_tier"].as_str())
                .map_or(
                    HostActionSafetyTier::HardDeny,
                    HostActionSafetyTier::from_wire,
                )
        } else if self.input_kind == "raw_input" {
            match self.action.as_str() {
                "move" | "scroll" => HostActionSafetyTier::TaskGrant,
                "click" | "double_click" | "right_click" | "toggle" | "drag"
                    if self.is_task_granted_pointer_input() =>
                {
                    HostActionSafetyTier::TaskGrant
                }
                "click" | "double_click" | "right_click" | "toggle" | "drag" | "type"
                | "type_chars" | "set_text" | "set_value" | "set_checked" => {
                    HostActionSafetyTier::ActionConfirmation
                }
                _ => HostActionSafetyTier::HardDeny,
            }
        } else {
            HostActionSafetyTier::HardDeny
        };
        if self.secret_handle.is_some() && base_tier != HostActionSafetyTier::HardDeny {
            HostActionSafetyTier::ActionConfirmation
        } else {
            base_tier
        }
    }

    pub(super) fn uses_physical_keyboard(&self) -> bool {
        matches!(
            self.action.as_str(),
            "keypress" | "press" | "press_key" | "keyboard_shortcut" | "hotkey"
        ) || !self.keys.is_empty()
            || !self.modifiers.is_empty()
    }

    fn is_task_granted_pointer_input(&self) -> bool {
        if !matches!(self.intent.as_str(), "navigate" | "ordinary_edit")
            || self.delivery_mode.as_deref() != Some("foreground")
            || self.text.is_some()
            || self.secret_handle.is_some()
            || !self.modifiers.is_empty()
        {
            return false;
        }
        match self.action.as_str() {
            "click" | "double_click" | "right_click" | "toggle" => {
                self.x.is_some() && self.y.is_some() && self.keys.is_empty()
            }
            "drag" => self.path.len() >= 2 && self.keys.is_empty(),
            _ => false,
        }
    }

    fn is_task_granted_keyboard_input(&self) -> bool {
        if !matches!(self.intent.as_str(), "navigate" | "ordinary_edit")
            || self.delivery_mode.as_deref() != Some("foreground")
            || self.text.is_some()
            || self.secret_handle.is_some()
            || self.input_backend_id.is_some()
            || self.element_index.is_some()
            || self.element_token.is_some()
            || self.x.is_some()
            || self.y.is_some()
            || self.button.is_some()
            || self.scroll_x.is_some()
            || self.scroll_y.is_some()
            || self.scroll_by.is_some()
            || !self.path.is_empty()
            || self.keys.is_empty()
            || !self.modifiers.is_empty()
            || self.delay_ms.is_some()
            || self.type_chars_only
            || self.checked.is_some()
            || self.steps.is_some()
        {
            return false;
        }
        if let Some(duration_ms) = self.duration_ms {
            return (1..=10_000).contains(&duration_ms)
                && matches!(self.action.as_str(), "keypress" | "press" | "press_key")
                && self.keys.len() <= 2
                && self.keys.iter().all(|key| is_safe_movement_key(key))
                && self.keys.iter().enumerate().all(|(index, key)| {
                    self.keys[..index]
                        .iter()
                        .all(|previous| !previous.trim().eq_ignore_ascii_case(key.trim()))
                });
        }
        self.keys.len() == 1 && is_safe_unmodified_key(&self.keys[0])
    }
}

fn is_safe_unmodified_key(key: &str) -> bool {
    let normalized = key.trim().to_ascii_uppercase();
    (normalized.len() == 1 && normalized.as_bytes()[0].is_ascii_alphanumeric())
        || matches!(
            normalized.as_str(),
            "ENTER"
                | "RETURN"
                | "TAB"
                | "ESCAPE"
                | "ESC"
                | "SPACE"
                | "UP"
                | "DOWN"
                | "LEFT"
                | "RIGHT"
                | "HOME"
                | "END"
                | "PAGEUP"
                | "PGUP"
                | "PAGEDOWN"
                | "PGDN"
        )
}

fn is_safe_movement_key(key: &str) -> bool {
    matches!(
        key.trim().to_ascii_uppercase().as_str(),
        "W" | "A" | "S" | "D" | "UP" | "DOWN" | "LEFT" | "RIGHT"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HostActionSafetyTier {
    HardDeny,
    ActionConfirmation,
    PreApproval,
    TaskGrant,
}

impl HostActionSafetyTier {
    fn from_wire(value: &str) -> Self {
        match value {
            "hard_deny" => Self::HardDeny,
            "action_confirmation" => Self::ActionConfirmation,
            "pre_approval" => Self::PreApproval,
            "task_grant" => Self::TaskGrant,
            _ => Self::HardDeny,
        }
    }

    pub(super) const fn requires_confirmation(self) -> bool {
        matches!(self, Self::ActionConfirmation | Self::PreApproval)
    }

    pub(super) const fn rejection(self) -> Option<(&'static str, &'static str, &'static str)> {
        match self {
            Self::HardDeny => Some((
                "hard_deny",
                "hard_denied",
                "the host policy denies this Computer Use action",
            )),
            _ => None,
        }
    }
}
