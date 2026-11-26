#[repr(u8)]
#[derive(Clone, Copy)]
pub(super) enum BannerStartupStage {
    ThreadStarted,
    DpiAwareness,
    TargetValidation,
    TargetIcon,
    OverlayCreation,
    FrameCreation,
    TargetGeometry,
    MonitorGeometry,
    BannerPlacement,
    FramePlacement,
    ReadinessSignal,
}

impl BannerStartupStage {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::ThreadStarted => "thread_started",
            Self::DpiAwareness => "dpi_awareness",
            Self::TargetValidation => "target_validation",
            Self::TargetIcon => "target_icon",
            Self::OverlayCreation => "overlay_creation",
            Self::FrameCreation => "frame_creation",
            Self::TargetGeometry => "target_geometry",
            Self::MonitorGeometry => "monitor_geometry",
            Self::BannerPlacement => "banner_placement",
            Self::FramePlacement => "frame_placement",
            Self::ReadinessSignal => "readiness_signal",
        }
    }

    pub(super) fn from_code(code: u8) -> Self {
        match code {
            0 => Self::ThreadStarted,
            1 => Self::DpiAwareness,
            2 => Self::TargetValidation,
            3 => Self::TargetIcon,
            4 => Self::OverlayCreation,
            5 => Self::FrameCreation,
            6 => Self::TargetGeometry,
            7 => Self::MonitorGeometry,
            8 => Self::BannerPlacement,
            9 => Self::FramePlacement,
            _ => Self::ReadinessSignal,
        }
    }
}
