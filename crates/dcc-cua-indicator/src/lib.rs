//! User-visible control-session indicator owned by the Host.
//!
//! CUA owns native input and its semantic mouse cursor. This crate owns the
//! separate, non-activating safety banner that tells the operator which app is
//! under agent control and provides the Escape stop boundary.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[allow(dead_code)]
mod theme_tokens {
    include!(concat!(env!("OUT_DIR"), "/theme_tokens.rs"));
}

const MAX_DISPLAY_NAME_CHARS: usize = 80;
/// Nominal target-frame thickness in device-independent pixels.
///
/// Windows scales this value using the exact target window's current monitor
/// DPI, preserving the same visual thickness when the target crosses displays.
pub const TARGET_FRAME_THICKNESS_DIP: i32 = theme_tokens::FRAME_THICKNESS_DIP;
/// Number of bands used to approximate the target-frame gradient.
pub const TARGET_FRAME_GRADIENT_STEPS: usize = theme_tokens::FRAME_GRADIENT_STEPS;
/// Duration of one complete target-frame breathing cycle.
pub const TARGET_FRAME_PULSE_PERIOD: Duration =
    Duration::from_millis(theme_tokens::FRAME_PULSE_PERIOD_MS);
/// Lowest target-frame opacity during a breathing cycle.
pub const TARGET_FRAME_ALPHA_MIN: u8 = theme_tokens::FRAME_ALPHA_MIN;
/// Highest target-frame opacity during a breathing cycle.
pub const TARGET_FRAME_ALPHA_MAX: u8 = theme_tokens::FRAME_ALPHA_MAX;
/// Cursor theme selected by the shared DCC CUA product-theme contract.
pub const SHARED_CURSOR_THEME_ID: &str = theme_tokens::CURSOR_THEME_ID;
/// Reduced-motion policy selected by the shared DCC CUA product-theme contract.
pub const SHARED_REDUCED_MOTION_POLICY: &str = theme_tokens::REDUCED_MOTION;
static INTERRUPT_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Per-session target-frame motion preference.
///
/// `Auto` preserves the accessible default by following the operating-system
/// animation preference. Overrides apply only to this control session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndicatorMotionPolicy {
    #[default]
    Auto,
    Reduce,
    Animate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolvedIndicatorMotion {
    Reduce,
    Animate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IndicatorMotionSource {
    SystemPreference,
    SessionOverride,
    SafeFallback,
    PlatformUnavailable,
}

/// Auditable resolution of a requested per-session indicator motion policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct IndicatorMotionStatus {
    pub requested: IndicatorMotionPolicy,
    pub resolved: ResolvedIndicatorMotion,
    pub motion_enabled: bool,
    pub source: IndicatorMotionSource,
}

impl IndicatorMotionStatus {
    #[must_use]
    pub const fn resolve(requested: IndicatorMotionPolicy, system_animations: bool) -> Self {
        match requested {
            IndicatorMotionPolicy::Auto => Self::from_enabled(
                requested,
                system_animations,
                IndicatorMotionSource::SystemPreference,
            ),
            IndicatorMotionPolicy::Reduce => {
                Self::from_enabled(requested, false, IndicatorMotionSource::SessionOverride)
            }
            IndicatorMotionPolicy::Animate => {
                Self::from_enabled(requested, true, IndicatorMotionSource::SessionOverride)
            }
        }
    }

    #[must_use]
    pub const fn resolve_from_system(
        requested: IndicatorMotionPolicy,
        system_animations: Option<bool>,
    ) -> Self {
        match requested {
            IndicatorMotionPolicy::Auto => match system_animations {
                Some(enabled) => Self::resolve(requested, enabled),
                None => Self::from_enabled(requested, false, IndicatorMotionSource::SafeFallback),
            },
            IndicatorMotionPolicy::Reduce => Self::resolve(requested, false),
            IndicatorMotionPolicy::Animate => Self::resolve(requested, true),
        }
    }

    #[must_use]
    pub const fn platform_unavailable(requested: IndicatorMotionPolicy) -> Self {
        Self::from_enabled(requested, false, IndicatorMotionSource::PlatformUnavailable)
    }

    const fn from_enabled(
        requested: IndicatorMotionPolicy,
        motion_enabled: bool,
        source: IndicatorMotionSource,
    ) -> Self {
        Self {
            requested,
            resolved: if motion_enabled {
                ResolvedIndicatorMotion::Animate
            } else {
                ResolvedIndicatorMotion::Reduce
            },
            motion_enabled,
            source,
        }
    }
}

#[must_use]
/// Compute the breathing opacity for a point in the shared theme cycle.
pub fn breathing_frame_alpha(elapsed: Duration) -> u8 {
    let phase = elapsed.as_secs_f64() / TARGET_FRAME_PULSE_PERIOD.as_secs_f64();
    let wave = (phase * std::f64::consts::TAU).cos().mul_add(0.5, 0.5);
    f64::from(TARGET_FRAME_ALPHA_MIN)
        .mul_add(1.0 - wave, f64::from(TARGET_FRAME_ALPHA_MAX) * wave)
        .round() as u8
}

#[cfg(any(test, windows))]
#[must_use]
pub(crate) fn indicator_frame_alpha(motion: IndicatorMotionStatus, elapsed: Duration) -> u8 {
    if motion.motion_enabled {
        breathing_frame_alpha(elapsed)
    } else {
        TARGET_FRAME_ALPHA_MAX
    }
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

#[cfg(any(test, windows))]
pub(crate) fn visible_target_frame_band(thickness: i32, band: usize) -> Option<(i32, i32)> {
    target_frame_band_insets(thickness, band).filter(|(outer, inner)| outer < inner)
}

#[cfg(any(test, windows))]
#[must_use]
pub(crate) fn target_frame_has_visible_band(thickness: i32) -> bool {
    (0..TARGET_FRAME_GRADIENT_STEPS)
        .any(|band| visible_target_frame_band(thickness, band).is_some())
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

/// Cursor accent selected by the shared DCC CUA product-theme contract.
pub const SHARED_CURSOR_ACCENT: BannerColor = color_from_token(theme_tokens::CURSOR_ACCENT);

const fn color_from_token((red, green, blue): (u8, u8, u8)) -> BannerColor {
    BannerColor { red, green, blue }
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
    Operating,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BannerBadge {
    Recording,
    LiveObservation,
}

/// Long-lived control signals that remain true while the current operation changes.
///
/// Recording and continuous observation are capabilities of the session, not
/// mutually-exclusive activities. Keeping them separate prevents a click or a
/// screenshot from hiding the fact that showcase recording is still active.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct BannerIndicators {
    pub recording: bool,
    pub live_observation: bool,
}

impl BannerIndicators {
    #[must_use]
    pub const fn badges(self) -> [Option<BannerBadge>; 2] {
        [
            if self.recording {
                Some(BannerBadge::Recording)
            } else {
                None
            },
            if self.live_observation {
                Some(BannerBadge::LiveObservation)
            } else {
                None
            },
        ]
    }
}

impl BannerActivity {
    /// Project an operation and the persistent session indicators into the
    /// primary label shown by the compact banner.
    #[must_use]
    pub const fn presented_with(self, indicators: BannerIndicators) -> Self {
        if !matches!(self, Self::Ready) {
            return self;
        }
        if indicators.live_observation {
            Self::Observing
        } else if indicators.recording {
            Self::Recording
        } else {
            Self::Ready
        }
    }

    #[must_use]
    pub const fn color(self) -> BannerColor {
        match self {
            Self::Connecting | Self::Observing | Self::Navigating => {
                color_from_token(theme_tokens::STATUS_INFORMATION)
            }
            Self::Ready => color_from_token(theme_tokens::STATUS_READY),
            Self::PointerInput | Self::KeyboardInput | Self::Waiting => {
                color_from_token(theme_tokens::STATUS_ACTION)
            }
            Self::Operating => color_from_token(theme_tokens::STATUS_ACTION),
            Self::Recording => color_from_token(theme_tokens::STATUS_RECORDING),
            Self::Stopping => color_from_token(theme_tokens::STATUS_STOPPING),
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
            9 => Self::Operating,
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
                Self::Operating => "正在操作",
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
                Self::Operating => "Operating",
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
    pub healthy: bool,
    pub running: bool,
    pub last_error: Option<String>,
    pub failure: Option<BannerFailure>,
    pub visible: bool,
    pub target_frame_visible: bool,
    pub interrupted: bool,
    pub stop_key: &'static str,
    pub label: String,
    pub activity: BannerActivity,
    pub activity_label: String,
    pub recording: bool,
    pub live_observation: bool,
    pub placement: &'static str,
    pub color: BannerColor,
    pub motion: IndicatorMotionStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BannerFailureKind {
    TargetLost,
    Backend,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BannerFailure {
    pub kind: BannerFailureKind,
    pub message: String,
}

#[derive(Debug, Clone, Error)]
pub enum IndicatorError {
    #[error("invalid banner target: {0}")]
    InvalidTarget(String),
    #[error("control banner backend failed: {0}")]
    Backend(String),
}

impl From<&IndicatorError> for BannerFailure {
    fn from(error: &IndicatorError) -> Self {
        Self {
            kind: match error {
                IndicatorError::InvalidTarget(_) => BannerFailureKind::TargetLost,
                IndicatorError::Backend(_) => BannerFailureKind::Backend,
            },
            message: error.to_string(),
        }
    }
}

pub struct ControlBanner {
    generation: u64,
    label: String,
    platform: platform::PlatformBanner,
}

const ACTIVITY_CODE_BITS: u32 = 8;
const ACTIVITY_CODE_MASK: u64 = (1 << ACTIVITY_CODE_BITS) - 1;

struct BannerActivitySignal {
    state: AtomicU64,
}

impl BannerActivitySignal {
    fn new(activity: BannerActivity) -> Self {
        Self {
            state: AtomicU64::new(u64::from(activity as u8)),
        }
    }

    fn load(&self) -> BannerActivity {
        BannerActivity::from_code((self.state.load(Ordering::Acquire) & ACTIVITY_CODE_MASK) as u8)
    }

    fn set(&self, activity: BannerActivity) -> u64 {
        let mut current = self.state.load(Ordering::Acquire);
        loop {
            let generation = (current >> ACTIVITY_CODE_BITS).wrapping_add(1);
            let next = (generation << ACTIVITY_CODE_BITS) | u64::from(activity as u8);
            match self.state.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return next,
                Err(observed) => current = observed,
            }
        }
    }

    fn clear_if_current(&self, token: u64) {
        let ready = (token & !ACTIVITY_CODE_MASK) | u64::from(BannerActivity::Ready as u8);
        let _ = self
            .state
            .compare_exchange(token, ready, Ordering::AcqRel, Ordering::Acquire);
    }
}

/// Restores the banner to its projected idle state on every return path.
///
/// The guard owns the shared activity signal rather than borrowing the banner,
/// so callers can safely keep it across async suspension points.
#[must_use = "keep the guard alive for the duration of the visible operation"]
pub struct BannerActivityGuard {
    activity: Arc<BannerActivitySignal>,
    token: u64,
}

impl BannerActivityGuard {
    fn begin(activity: Arc<BannerActivitySignal>, next: BannerActivity) -> Self {
        let token = activity.set(next);
        Self { activity, token }
    }
}

impl Drop for BannerActivityGuard {
    fn drop(&mut self) {
        self.activity.clear_if_current(self.token);
    }
}

impl ControlBanner {
    pub fn start(target: BannerTarget) -> Result<Self, IndicatorError> {
        Self::start_with_motion(target, IndicatorMotionPolicy::Auto)
    }

    pub fn start_with_motion(
        target: BannerTarget,
        motion: IndicatorMotionPolicy,
    ) -> Result<Self, IndicatorError> {
        target.validate()?;
        let generation = interrupt_generation();
        let label = target.identity();
        let platform = platform::PlatformBanner::start(target, motion, generation)?;
        Ok(Self {
            generation,
            label,
            platform,
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

    #[must_use]
    pub fn failure(&self) -> Option<BannerFailure> {
        let status = self.status();
        if status.backend == "unavailable"
            || status.interrupted
            || (status.healthy && status.running)
        {
            return None;
        }
        status.failure.or_else(|| {
            Some(BannerFailure {
                kind: BannerFailureKind::Backend,
                message: status
                    .last_error
                    .unwrap_or_else(|| "control-banner presenter stopped unexpectedly".into()),
            })
        })
    }

    pub fn set_activity(&self, activity: BannerActivity) {
        self.platform.set_activity(activity);
    }

    pub fn begin_activity(&self, activity: BannerActivity) -> BannerActivityGuard {
        BannerActivityGuard::begin(self.platform.activity_handle(), activity)
    }

    pub fn set_recording(&self, recording: bool) {
        self.platform.set_recording(recording);
    }

    pub fn set_live_observation(&self, live_observation: bool) {
        self.platform.set_live_observation(live_observation);
    }
}

#[cfg(windows)]
mod platform;

#[cfg(not(windows))]
mod platform {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::{
        BannerActivity, BannerActivitySignal, BannerFailure, BannerFailureKind, BannerIndicators,
        BannerStatus, BannerTarget, IndicatorError, IndicatorMotionPolicy, IndicatorMotionStatus,
        system_language_tag,
    };

    pub(super) struct PlatformBanner {
        activity: Arc<BannerActivitySignal>,
        recording: AtomicBool,
        live_observation: AtomicBool,
        motion: IndicatorMotionStatus,
    }

    impl PlatformBanner {
        pub(super) fn start(
            _target: BannerTarget,
            motion: IndicatorMotionPolicy,
            _generation: u64,
        ) -> Result<Self, IndicatorError> {
            Ok(Self {
                activity: Arc::new(BannerActivitySignal::new(BannerActivity::Ready)),
                recording: AtomicBool::new(false),
                live_observation: AtomicBool::new(false),
                motion: IndicatorMotionStatus::platform_unavailable(motion),
            })
        }

        pub(super) fn status(&self) -> BannerStatus {
            let indicators = BannerIndicators {
                recording: self.recording.load(Ordering::Acquire),
                live_observation: self.live_observation.load(Ordering::Acquire),
            };
            let activity = self.activity.load().presented_with(indicators);
            BannerStatus {
                backend: "unavailable",
                healthy: false,
                running: false,
                last_error: Some("no native control-banner presenter is available".into()),
                failure: Some(BannerFailure {
                    kind: BannerFailureKind::Backend,
                    message: "no native control-banner presenter is available".into(),
                }),
                visible: false,
                target_frame_visible: false,
                interrupted: false,
                stop_key: "Escape",
                label: String::new(),
                activity,
                activity_label: activity.localized_label(&system_language_tag()).into(),
                recording: indicators.recording,
                live_observation: indicators.live_observation,
                placement: "unavailable",
                color: activity.color(),
                motion: self.motion,
            }
        }

        pub(super) fn interrupted(&self) -> bool {
            false
        }

        pub(super) fn set_activity(&self, activity: BannerActivity) {
            self.activity.set(activity);
        }

        pub(super) fn activity_handle(&self) -> Arc<BannerActivitySignal> {
            Arc::clone(&self.activity)
        }

        pub(super) fn set_recording(&self, recording: bool) {
            self.recording.store(recording, Ordering::Release);
        }

        pub(super) fn set_live_observation(&self, live_observation: bool) {
            self.live_observation
                .store(live_observation, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests;
