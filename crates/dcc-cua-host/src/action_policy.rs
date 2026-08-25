use serde_json::Value;

use super::HostAction;

impl HostAction {
    pub(super) fn safety_tier(&self, accessibility_root: Option<&Value>) -> HostActionSafetyTier {
        let base_tier = if self.input_kind == "semantic" {
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
                    if self.is_task_granted_navigation_input() =>
                {
                    HostActionSafetyTier::TaskGrant
                }
                "click" | "double_click" | "right_click" | "toggle" | "drag" | "type"
                | "type_chars" | "set_text" | "set_value" | "set_checked" | "keypress"
                | "press" | "press_key" | "keyboard_shortcut" | "hotkey" => {
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

    fn is_task_granted_navigation_input(&self) -> bool {
        if self.intent != "navigate"
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
