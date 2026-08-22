use std::sync::Mutex;

use windows::Win32::Foundation::{COLORREF, HANDLE, HWND};
use windows::Win32::UI::WindowsAndMessaging::{LWA_ALPHA, SetLayeredWindowAttributes, SetPropW};

use super::super::{BannerActivity, BannerFailure, BannerFailureKind, IndicatorError};
use super::OVERLAY_ACTIVITY_PROP;

pub(super) fn set_activity_property(
    window: HWND,
    activity: BannerActivity,
) -> Result<(), IndicatorError> {
    unsafe {
        SetPropW(
            window,
            OVERLAY_ACTIVITY_PROP,
            Some(HANDLE(
                (usize::from(activity as u8) + 1) as *mut core::ffi::c_void,
            )),
        )
    }
    .map_err(|error| IndicatorError::Rendering(format!("set banner activity: {error}")))
}

pub(super) fn set_overlay_alpha(window: HWND, alpha: u8) -> Result<(), IndicatorError> {
    unsafe { SetLayeredWindowAttributes(window, COLORREF(0), alpha, LWA_ALPHA) }
        .map_err(|error| IndicatorError::Rendering(format!("set indicator opacity: {error}")))
}

pub(super) fn record_rendering_result(
    failure: &Mutex<Option<BannerFailure>>,
    result: Result<(), IndicatorError>,
) -> bool {
    match result {
        Ok(()) => true,
        Err(error) => {
            let failure_value = BannerFailure::from(&error);
            debug_assert_eq!(failure_value.kind, BannerFailureKind::Rendering);
            if let Ok(mut stored) = failure.lock() {
                *stored = Some(failure_value);
            }
            false
        }
    }
}

pub(super) fn record_rendering_results(
    failure: &Mutex<Option<BannerFailure>>,
    results: impl IntoIterator<Item = Result<(), IndicatorError>>,
) -> bool {
    let mut all_rendered = true;
    for result in results {
        all_rendered = record_rendering_result(failure, result) && all_rendered;
    }
    all_rendered
}
