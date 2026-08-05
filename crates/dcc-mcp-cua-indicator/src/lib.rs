//! User-visible control-session indicator owned by the Host.
//!
//! CUA owns native input and its semantic mouse cursor. This crate owns the
//! separate, non-activating safety banner that tells the operator which app is
//! under agent control and provides the Escape stop boundary.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use thiserror::Error;

const MAX_LABEL_CHARS: usize = 512;
const MAX_DISPLAY_NAME_CHARS: usize = 80;
// Win32 COLORREF is BGR; DEFAULT is RGB (10, 132, 255), whose hue is ~209°.
const DEFAULT_HUE_DEGREES: u16 = 209;
static INTERRUPT_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Broadcast a cooperative stop to every control session in this Host process.
pub fn broadcast_interrupt() -> u64 {
    INTERRUPT_GENERATION
        .fetch_add(1, Ordering::AcqRel)
        .wrapping_add(1)
}

/// Return the current Host-process stop generation.
#[must_use]
pub fn interrupt_generation() -> u64 {
    INTERRUPT_GENERATION.load(Ordering::Acquire)
}

#[must_use]
pub fn interrupt_generation_changed(started: u64, current: u64) -> bool {
    started != current
}

#[derive(Debug, Clone)]
pub struct BannerTarget {
    pub process_id: u32,
    pub window_handle: u64,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BannerColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl BannerColor {
    pub const DEFAULT: Self = Self {
        red: 10,
        green: 132,
        blue: 255,
    };

    #[must_use]
    pub fn from_hue(hue: u16) -> Self {
        Self::from_hsv(hue, 0.78, 0.9)
    }

    #[must_use]
    fn from_hsv(hue: u16, saturation: f32, value: f32) -> Self {
        let hue = f32::from(hue % 360) / 60.0;
        let chroma = value * saturation;
        let x = chroma * (1.0 - ((hue % 2.0) - 1.0).abs());
        let m = value - chroma;
        let (red, green, blue) = match hue as u32 {
            0 => (chroma, x, 0.0),
            1 => (x, chroma, 0.0),
            2 => (0.0, chroma, x),
            3 => (0.0, x, chroma),
            4 => (x, 0.0, chroma),
            _ => (chroma, 0.0, x),
        };
        Self {
            red: ((red + m) * 255.0).round() as u8,
            green: ((green + m) * 255.0).round() as u8,
            blue: ((blue + m) * 255.0).round() as u8,
        }
    }

    #[must_use]
    pub fn frame(self) -> Self {
        Self::from_hsv(self.hue(), 0.55, 0.98)
    }

    #[must_use]
    pub fn colorref(self) -> u32 {
        (u32::from(self.blue) << 16) | (u32::from(self.green) << 8) | u32::from(self.red)
    }

    #[must_use]
    pub fn hue(self) -> u16 {
        let red = f32::from(self.red) / 255.0;
        let green = f32::from(self.green) / 255.0;
        let blue = f32::from(self.blue) / 255.0;
        let maximum = red.max(green).max(blue);
        let minimum = red.min(green).min(blue);
        let delta = maximum - minimum;
        if delta == 0.0 {
            return 0;
        }
        let hue = if maximum == red {
            60.0 * ((green - blue) / delta).rem_euclid(6.0)
        } else if maximum == green {
            60.0 * ((blue - red) / delta + 2.0)
        } else {
            60.0 * ((red - green) / delta + 4.0)
        };
        hue.round() as u16 % 360
    }
}

/// Generate a stable-but-session-random color, excluding the default banner hue.
#[must_use]
pub fn session_color(agent_name: &str, session_id: &str) -> BannerColor {
    let mut hasher = DefaultHasher::new();
    agent_name.hash(&mut hasher);
    session_id.hash(&mut hasher);
    let hue = (hasher.finish() % 360) as u16;
    let hue = if hue.abs_diff(DEFAULT_HUE_DEGREES) < 45 {
        (hue + 180) % 360
    } else {
        hue
    };
    BannerColor::from_hue(hue)
}

/// Build the human-facing control label in the operating system language.
#[must_use]
pub fn localized_control_label(agent_name: &str, app_name: &str) -> String {
    localized_control_label_for_language(&system_language_tag(), agent_name, app_name)
}

fn localized_control_label_for_language(
    language_tag: &str,
    agent_name: &str,
    app_name: &str,
) -> String {
    let agent = display_name(agent_name, "Agent");
    let app = display_name(app_name, "application");
    match language_tag
        .split(['-', '_'])
        .next()
        .unwrap_or("en")
        .to_ascii_lowercase()
        .as_str()
    {
        "zh" => format!("{agent} 正在操作 {app}"),
        "ja" => format!("{agent} が {app} を操作中"),
        "ko" => format!("{agent}이(가) {app}을(를) 조작 중"),
        "fr" => format!("{agent} contrôle {app}"),
        "de" => format!("{agent} steuert {app}"),
        "es" => format!("{agent} está controlando {app}"),
        _ => format!("{agent} is controlling {app}"),
    }
}

fn display_name(value: &str, fallback: &str) -> String {
    let value: String = value
        .trim()
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_DISPLAY_NAME_CHARS)
        .collect();
    if value.is_empty() {
        fallback.into()
    } else {
        value
    }
}

#[cfg(windows)]
fn system_language_tag() -> String {
    platform::system_language_tag()
}

#[cfg(not(windows))]
fn system_language_tag() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANGUAGE", "LANG"]
        .iter()
        .find_map(|key| std::env::var(key).ok().filter(|value| !value.is_empty()))
        .unwrap_or_else(|| "en".into())
}

impl BannerTarget {
    fn validate(&self) -> Result<(), IndicatorError> {
        if self.process_id == 0 || self.window_handle == 0 {
            return Err(IndicatorError::InvalidTarget(
                "control banner requires an exact process and window".into(),
            ));
        }
        let label_len = self.label.chars().count();
        if label_len == 0 || label_len > MAX_LABEL_CHARS {
            return Err(IndicatorError::InvalidTarget(format!(
                "control banner label must contain 1..{MAX_LABEL_CHARS} characters"
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BannerStatus {
    pub backend: &'static str,
    pub visible: bool,
    pub target_frame_visible: bool,
    pub interrupted: bool,
    pub stop_key: &'static str,
    pub label: String,
    pub color: BannerColor,
}

#[derive(Debug, Error)]
pub enum IndicatorError {
    #[error("invalid banner target: {0}")]
    InvalidTarget(String),
    #[error("control banner backend failed: {0}")]
    Backend(String),
}

pub struct ControlBanner {
    generation: u64,
    label: String,
    color: BannerColor,
    platform: platform::PlatformBanner,
}

impl ControlBanner {
    pub fn start(target: BannerTarget) -> Result<Self, IndicatorError> {
        Self::start_with_color(target, BannerColor::DEFAULT)
    }

    pub fn start_with_color(
        target: BannerTarget,
        color: BannerColor,
    ) -> Result<Self, IndicatorError> {
        target.validate()?;
        Ok(Self {
            generation: interrupt_generation(),
            label: target.label.clone(),
            color,
            platform: platform::PlatformBanner::start(target, color)?,
        })
    }

    #[must_use]
    pub fn status(&self) -> BannerStatus {
        let mut status = self.platform.status();
        status.interrupted = self.interrupted();
        status.label = self.label.clone();
        status.color = self.color;
        status
    }

    #[must_use]
    pub fn interrupted(&self) -> bool {
        interrupt_generation_changed(self.generation, interrupt_generation())
            || self.platform.interrupted()
    }

    pub fn set_cursor_position(&self, x: f64, y: f64) {
        self.platform.set_cursor_position(x, y);
    }
}

#[cfg(windows)]
mod platform;

#[cfg(not(windows))]
mod platform {
    use super::{BannerColor, BannerStatus, BannerTarget, IndicatorError};

    pub(super) struct PlatformBanner;

    impl PlatformBanner {
        pub(super) fn start(
            _target: BannerTarget,
            _color: BannerColor,
        ) -> Result<Self, IndicatorError> {
            Ok(Self)
        }

        pub(super) fn status(&self) -> BannerStatus {
            BannerStatus {
                backend: "unavailable",
                visible: false,
                target_frame_visible: false,
                interrupted: false,
                stop_key: "Escape",
                label: String::new(),
                color: BannerColor::DEFAULT,
            }
        }

        pub(super) fn interrupted(&self) -> bool {
            false
        }

        pub(super) fn set_cursor_position(&self, _x: f64, _y: f64) {}
    }
}

#[cfg(test)]
mod tests;
