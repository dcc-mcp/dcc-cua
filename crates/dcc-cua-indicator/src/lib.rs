//! User-visible control-session indicator owned by the Host.
//!
//! CUA owns native input and its semantic mouse cursor. This crate owns the
//! separate, non-activating safety banner that tells the operator which app is
//! under agent control and provides the Escape stop boundary.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::Serialize;
use thiserror::Error;

const MAX_DISPLAY_NAME_CHARS: usize = 80;
/// Default physical target-frame thickness in device-independent pixels.
pub const TARGET_FRAME_THICKNESS_DIP: i32 = 40;
/// Number of bands used to approximate the target-frame gradient.
pub const TARGET_FRAME_GRADIENT_STEPS: usize = 20;
/// Duration of one complete target-frame breathing cycle.
pub const TARGET_FRAME_PULSE_PERIOD: Duration = Duration::from_millis(1_800);
/// Lowest target-frame opacity during a breathing cycle.
pub const TARGET_FRAME_ALPHA_MIN: u8 = 48;
/// Highest target-frame opacity during a breathing cycle.
pub const TARGET_FRAME_ALPHA_MAX: u8 = 132;
static INTERRUPT_GENERATION: AtomicU64 = AtomicU64::new(0);

#[must_use]
/// Compute the breathing opacity for a point in the shared theme cycle.
pub fn breathing_frame_alpha(elapsed: Duration) -> u8 {
    let phase = elapsed.as_secs_f64() / TARGET_FRAME_PULSE_PERIOD.as_secs_f64();
    let wave = (phase * std::f64::consts::TAU).cos().mul_add(0.5, 0.5);
    f64::from(TARGET_FRAME_ALPHA_MIN)
        .mul_add(1.0 - wave, f64::from(TARGET_FRAME_ALPHA_MAX) * wave)
        .round() as u8
}

#[must_use]
/// Compute the opacity of one gradient band from the current edge opacity.
pub fn target_frame_band_alpha(alpha: u8, band: usize) -> u8 {
    let remaining = TARGET_FRAME_GRADIENT_STEPS
        .saturating_sub(band)
        .min(TARGET_FRAME_GRADIENT_STEPS);
    let divisor = TARGET_FRAME_GRADIENT_STEPS * TARGET_FRAME_GRADIENT_STEPS;
    (usize::from(alpha) * remaining * remaining / divisor) as u8
}

#[must_use]
/// Compute the outer and inner inset of one gradient band.
pub fn target_frame_band_insets(thickness: i32, band: usize) -> Option<(i32, i32)> {
    if thickness <= 0 || band >= TARGET_FRAME_GRADIENT_STEPS {
        return None;
    }
    let steps = TARGET_FRAME_GRADIENT_STEPS as i32;
    let outer = thickness * band as i32 / steps;
    let inner = thickness * (band as i32 + 1) / steps;
    Some((outer, inner))
}

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
    pub agent_name: String,
    pub application_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BannerColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum BannerActivity {
    Connecting,
    #[default]
    Ready,
    Observing,
    PointerInput,
    KeyboardInput,
    Navigating,
    Waiting,
    Recording,
    Stopping,
}

impl BannerActivity {
    #[must_use]
    pub const fn color(self) -> BannerColor {
        match self {
            Self::Connecting | Self::Observing | Self::Navigating => BannerColor {
                red: 110,
                green: 182,
                blue: 255,
            },
            Self::Ready => BannerColor {
                red: 115,
                green: 215,
                blue: 167,
            },
            Self::PointerInput | Self::KeyboardInput | Self::Waiting => BannerColor {
                red: 243,
                green: 201,
                blue: 107,
            },
            Self::Recording => BannerColor {
                red: 255,
                green: 137,
                blue: 137,
            },
            Self::Stopping => BannerColor {
                red: 127,
                green: 137,
                blue: 148,
            },
        }
    }

    #[must_use]
    pub(crate) const fn from_code(code: u8) -> Self {
        match code {
            0 => Self::Connecting,
            2 => Self::Observing,
            3 => Self::PointerInput,
            4 => Self::KeyboardInput,
            5 => Self::Navigating,
            6 => Self::Waiting,
            7 => Self::Recording,
            8 => Self::Stopping,
            _ => Self::Ready,
        }
    }

    #[must_use]
    pub(crate) fn localized_label(self, language_tag: &str) -> &'static str {
        let chinese = language_tag
            .split(['-', '_'])
            .next()
            .is_some_and(|language| language.eq_ignore_ascii_case("zh"));
        if chinese {
            match self {
                Self::Connecting => "正在连接…",
                Self::Ready => "已连接 · 等待操作",
                Self::Observing => "正在观察画面",
                Self::PointerInput => "正在使用鼠标",
                Self::KeyboardInput => "正在输入文本",
                Self::Navigating => "正在切换界面",
                Self::Waiting => "正在等待应用",
                Self::Recording => "正在录制",
                Self::Stopping => "正在停止…",
            }
        } else {
            match self {
                Self::Connecting => "Connecting…",
                Self::Ready => "Connected · Ready",
                Self::Observing => "Observing screen",
                Self::PointerInput => "Using pointer",
                Self::KeyboardInput => "Entering text",
                Self::Navigating => "Navigating",
                Self::Waiting => "Waiting for application",
                Self::Recording => "Recording",
                Self::Stopping => "Stopping…",
            }
        }
    }
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
        display_name(&self.agent_name, "Agent");
        display_name(&self.application_name, "application");
        Ok(())
    }

    fn identity(&self) -> String {
        format!(
            "{} · {}",
            display_name(&self.agent_name, "Agent"),
            display_name(&self.application_name, "application")
        )
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
    pub activity: BannerActivity,
    pub activity_label: String,
    pub placement: &'static str,
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
    platform: platform::PlatformBanner,
}

impl ControlBanner {
    pub fn start(target: BannerTarget) -> Result<Self, IndicatorError> {
        target.validate()?;
        Ok(Self {
            generation: interrupt_generation(),
            label: target.identity(),
            platform: platform::PlatformBanner::start(target)?,
        })
    }

    #[must_use]
    pub fn status(&self) -> BannerStatus {
        let mut status = self.platform.status();
        status.interrupted = self.interrupted();
        status.label = self.label.clone();
        status
    }

    #[must_use]
    pub fn interrupted(&self) -> bool {
        interrupt_generation_changed(self.generation, interrupt_generation())
            || self.platform.interrupted()
    }

    pub fn set_activity(&self, activity: BannerActivity) {
        self.platform.set_activity(activity);
    }
}

#[cfg(windows)]
mod platform;

#[cfg(not(windows))]
mod platform {
    use std::sync::atomic::{AtomicU8, Ordering};

    use super::{BannerActivity, BannerStatus, BannerTarget, IndicatorError, system_language_tag};

    pub(super) struct PlatformBanner {
        activity: AtomicU8,
    }

    impl PlatformBanner {
        pub(super) fn start(_target: BannerTarget) -> Result<Self, IndicatorError> {
            Ok(Self {
                activity: AtomicU8::new(BannerActivity::Ready as u8),
            })
        }

        pub(super) fn status(&self) -> BannerStatus {
            let activity = BannerActivity::from_code(self.activity.load(Ordering::Acquire));
            BannerStatus {
                backend: "unavailable",
                visible: false,
                target_frame_visible: false,
                interrupted: false,
                stop_key: "Escape",
                label: String::new(),
                activity,
                activity_label: activity.localized_label(&system_language_tag()).into(),
                placement: "unavailable",
                color: activity.color(),
            }
        }

        pub(super) fn interrupted(&self) -> bool {
            false
        }

        pub(super) fn set_activity(&self, activity: BannerActivity) {
            self.activity.store(activity as u8, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests;
