//! Bounded, content-free display metadata for the explicit capture diagnostic.
//! This module never reads pixels, changes display settings, or authorizes input.
use serde::Serialize;
use windows::Win32::{
    Devices::Display::*,
    Foundation::{ERROR_INSUFFICIENT_BUFFER, GetLastError, HWND, LUID},
    Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONULL, MONITORINFOEXW, MonitorFromWindow},
};

use crate::visible_capture::exact_window_instance_evidence;

const MAX_PATHS: u32 = 64;
const MAX_MODES: u32 = 128;
const MAX_QUERY_ATTEMPTS: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayColorStatus {
    Collected,
    TargetUnavailable,
    TargetChanged,
    MonitorUnavailable,
    DisplayConfigUnavailable,
    BoundsExceeded,
    SourceMappingUnavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NativeAdvancedColorInfo {
    pub raw_flags: u32,
    pub advanced_color_supported: bool,
    pub advanced_color_enabled: bool,
    pub wide_color_enforced: bool,
    pub advanced_color_force_disabled: bool,
    pub color_encoding: i32,
    pub bits_per_color_channel: u32,
}

impl NativeAdvancedColorInfo {
    fn decode(raw_flags: u32, color_encoding: i32, bits_per_color_channel: u32) -> Self {
        Self {
            raw_flags,
            advanced_color_supported: raw_flags & 1 != 0,
            advanced_color_enabled: raw_flags & 2 != 0,
            wide_color_enforced: raw_flags & 4 != 0,
            advanced_color_force_disabled: raw_flags & 8 != 0,
            color_encoding,
            bits_per_color_channel,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct DisplayConfigIdentity {
    pub adapter_low: u32,
    pub adapter_high: i32,
    pub id: u32,
}

impl DisplayConfigIdentity {
    fn new(adapter: LUID, id: u32) -> Self {
        Self {
            adapter_low: adapter.LowPart,
            adapter_high: adapter.HighPart,
            id,
        }
    }

    fn header<T>(self, kind: DISPLAYCONFIG_DEVICE_INFO_TYPE) -> DISPLAYCONFIG_DEVICE_INFO_HEADER {
        DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: kind,
            size: std::mem::size_of::<T>() as u32,
            adapterId: LUID {
                LowPart: self.adapter_low,
                HighPart: self.adapter_high,
            },
            id: self.id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NativeOutputColorProof {
    pub source: DisplayConfigIdentity,
    pub target: DisplayConfigIdentity,
    pub target_available: bool,
    pub advanced_color: Option<NativeAdvancedColorInfo>,
    pub advanced_color_error: Option<u32>,
    /// Native API value, a multiplier of 80 nits multiplied by 1000.
    pub sdr_white_level: Option<u32>,
    pub sdr_white_level_millinits: Option<u64>,
    pub sdr_white_level_error: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NativeDisplayColorProof {
    pub diagnostic_only: bool,
    pub configuration_stability_verified: bool,
    pub status: DisplayColorStatus,
    pub os_error: Option<u32>,
    pub monitor_bounds: Option<[i32; 4]>,
    pub active_path_count: Option<u32>,
    pub active_mode_count: Option<u32>,
    pub max_paths: u32,
    pub max_modes: u32,
    pub max_query_attempts: usize,
    pub source_mapping_complete: bool,
    pub source_query_errors: Vec<(u32, u32)>,
    /// Every successfully mapped active target, including cloned outputs.
    pub outputs: Vec<NativeOutputColorProof>,
}

impl NativeDisplayColorProof {
    fn empty(status: DisplayColorStatus) -> Self {
        Self {
            diagnostic_only: true,
            configuration_stability_verified: false,
            status,
            os_error: None,
            monitor_bounds: None,
            active_path_count: None,
            active_mode_count: None,
            max_paths: MAX_PATHS,
            max_modes: MAX_MODES,
            max_query_attempts: MAX_QUERY_ATTEMPTS,
            source_mapping_complete: false,
            source_query_errors: Vec::new(),
            outputs: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Monitor {
    name: [u16; 32],
    bounds: [i32; 4],
}

#[derive(Clone, Copy, Debug)]
struct Path {
    source: DisplayConfigIdentity,
    target: DisplayConfigIdentity,
    target_available: bool,
}

struct ActivePaths {
    paths: Vec<Path>,
    mode_count: u32,
}

struct Failure(DisplayColorStatus, Option<u32>);

trait DisplayColorReader {
    fn monitor(&mut self) -> Result<Monitor, Failure>;
    fn active_paths(&mut self) -> Result<ActivePaths, Failure>;
    fn source_name(&mut self, source: DisplayConfigIdentity) -> Result<[u16; 32], u32>;
    fn advanced_color(
        &mut self,
        target: DisplayConfigIdentity,
    ) -> Result<NativeAdvancedColorInfo, u32>;
    fn sdr_white_level(&mut self, target: DisplayConfigIdentity) -> Result<u32, u32>;
}

fn same_source_name(left: &[u16; 32], right: &[u16; 32]) -> bool {
    let left_end = left.iter().position(|value| *value == 0);
    let right_end = right.iter().position(|value| *value == 0);
    match (left_end, right_end) {
        (Some(a), Some(b)) if a != 0 && a == b => left[..a] == right[..b],
        _ => false,
    }
}

fn bounded_counts(paths: u32, modes: u32) -> bool {
    paths > 0 && paths <= MAX_PATHS && modes <= MAX_MODES
}

fn collect(reader: &mut impl DisplayColorReader) -> NativeDisplayColorProof {
    let mut report = NativeDisplayColorProof::empty(DisplayColorStatus::Collected);
    let monitor = match reader.monitor() {
        Ok(monitor) => monitor,
        Err(Failure(status, error)) => {
            report.status = status;
            report.os_error = error;
            return report;
        }
    };
    report.monitor_bounds = Some(monitor.bounds);
    let paths = match reader.active_paths() {
        Ok(paths) => paths,
        Err(Failure(status, error)) => {
            report.status = status;
            report.os_error = error;
            return report;
        }
    };
    if !bounded_counts(paths.paths.len() as u32, paths.mode_count) {
        report.status = DisplayColorStatus::BoundsExceeded;
        return report;
    }
    report.active_path_count = Some(paths.paths.len() as u32);
    report.active_mode_count = Some(paths.mode_count);
    for (index, path) in paths.paths.into_iter().enumerate() {
        let name = match reader.source_name(path.source) {
            Ok(name) => name,
            Err(error) => {
                report.source_query_errors.push((index as u32, error));
                continue;
            }
        };
        if !same_source_name(&monitor.name, &name) {
            continue;
        }
        let advanced = reader.advanced_color(path.target);
        let white = reader.sdr_white_level(path.target);
        report.outputs.push(NativeOutputColorProof {
            source: path.source,
            target: path.target,
            target_available: path.target_available,
            advanced_color: advanced.as_ref().ok().cloned(),
            advanced_color_error: advanced.err(),
            sdr_white_level: white.as_ref().ok().copied(),
            // Microsoft defines SDRWhiteLevel / 1000 * 80 nits. Retain exact integer units.
            sdr_white_level_millinits: white.as_ref().ok().map(|raw| u64::from(*raw) * 80),
            sdr_white_level_error: white.err(),
        });
    }
    if report.outputs.is_empty() {
        report.status = DisplayColorStatus::SourceMappingUnavailable;
    }
    report.source_mapping_complete = report.source_query_errors.is_empty();
    match reader.monitor() {
        Ok(after) if after == monitor => {}
        _ => {
            report.status = DisplayColorStatus::TargetChanged;
            report.outputs.clear();
        }
    }
    report
}

struct NativeReader(HWND);

impl DisplayColorReader for NativeReader {
    fn monitor(&mut self) -> Result<Monitor, Failure> {
        let monitor = unsafe { MonitorFromWindow(self.0, MONITOR_DEFAULTTONULL) };
        if monitor.0.is_null() {
            return Err(Failure(DisplayColorStatus::MonitorUnavailable, None));
        }
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if !unsafe { GetMonitorInfoW(monitor, &mut info.monitorInfo) }.as_bool() {
            return Err(Failure(
                DisplayColorStatus::MonitorUnavailable,
                Some(unsafe { GetLastError() }.0),
            ));
        }
        let rect = info.monitorInfo.rcMonitor;
        Ok(Monitor {
            name: info.szDevice,
            bounds: [
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
            ],
        })
    }

    fn active_paths(&mut self) -> Result<ActivePaths, Failure> {
        // https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-querydisplayconfig
        // Hotplug can change buffer requirements between these calls; retry only twice.
        for _ in 0..MAX_QUERY_ATTEMPTS {
            let (mut path_count, mut mode_count) = (0, 0);
            let size_error = unsafe {
                GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
            };
            if size_error.0 != 0 {
                return Err(Failure(
                    DisplayColorStatus::DisplayConfigUnavailable,
                    Some(size_error.0),
                ));
            }
            if !bounded_counts(path_count, mode_count) {
                return Err(Failure(DisplayColorStatus::BoundsExceeded, None));
            }
            let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
            let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
            let query_error = unsafe {
                QueryDisplayConfig(
                    QDC_ONLY_ACTIVE_PATHS,
                    &mut path_count,
                    paths.as_mut_ptr(),
                    &mut mode_count,
                    modes.as_mut_ptr(),
                    None,
                )
            };
            if query_error == ERROR_INSUFFICIENT_BUFFER {
                continue;
            }
            if query_error.0 != 0 {
                return Err(Failure(
                    DisplayColorStatus::DisplayConfigUnavailable,
                    Some(query_error.0),
                ));
            }
            if !bounded_counts(path_count, mode_count)
                || path_count as usize > paths.len()
                || mode_count as usize > modes.len()
            {
                return Err(Failure(DisplayColorStatus::BoundsExceeded, None));
            }
            paths.truncate(path_count as usize);
            return Ok(ActivePaths {
                paths: paths
                    .into_iter()
                    .map(|path| Path {
                        source: DisplayConfigIdentity::new(
                            path.sourceInfo.adapterId,
                            path.sourceInfo.id,
                        ),
                        target: DisplayConfigIdentity::new(
                            path.targetInfo.adapterId,
                            path.targetInfo.id,
                        ),
                        target_available: path.targetInfo.targetAvailable.as_bool(),
                    })
                    .collect(),
                mode_count,
            });
        }
        Err(Failure(
            DisplayColorStatus::DisplayConfigUnavailable,
            Some(ERROR_INSUFFICIENT_BUFFER.0),
        ))
    }

    fn source_name(&mut self, source: DisplayConfigIdentity) -> Result<[u16; 32], u32> {
        let mut request = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
            header: source.header::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>(
                DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
            ),
            ..Default::default()
        };
        let error = unsafe { DisplayConfigGetDeviceInfo(&mut request.header) };
        if error == 0 {
            Ok(request.viewGdiDeviceName)
        } else {
            Err(error as u32)
        }
    }

    fn advanced_color(
        &mut self,
        target: DisplayConfigIdentity,
    ) -> Result<NativeAdvancedColorInfo, u32> {
        // Retain raw flags and unknown encoding values. Advanced Color is not an HDR assertion.
        let mut request = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO {
            header: target.header::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>(
                DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
            ),
            ..Default::default()
        };
        let error = unsafe { DisplayConfigGetDeviceInfo(&mut request.header) };
        if error != 0 {
            return Err(error as u32);
        }
        Ok(NativeAdvancedColorInfo::decode(
            unsafe { request.Anonymous.value },
            request.colorEncoding.0,
            request.bitsPerColorChannel,
        ))
    }

    fn sdr_white_level(&mut self, target: DisplayConfigIdentity) -> Result<u32, u32> {
        // https://learn.microsoft.com/en-us/windows/win32/api/wingdi/ns-wingdi-displayconfig_sdr_white_level
        let mut request = DISPLAYCONFIG_SDR_WHITE_LEVEL {
            header: target.header::<DISPLAYCONFIG_SDR_WHITE_LEVEL>(
                DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL,
            ),
            ..Default::default()
        };
        let error = unsafe { DisplayConfigGetDeviceInfo(&mut request.header) };
        if error == 0 {
            Ok(request.SDRWhiteLevel)
        } else {
            Err(error as u32)
        }
    }
}

pub(crate) fn exact_window_display_color(
    process_id: u32,
    window_handle: u64,
) -> NativeDisplayColorProof {
    let before = match exact_window_instance_evidence(process_id, window_handle) {
        Ok(instance) => instance,
        Err(_) => return NativeDisplayColorProof::empty(DisplayColorStatus::TargetUnavailable),
    };
    let mut report = collect(&mut NativeReader(HWND(window_handle as usize as *mut _)));
    if exact_window_instance_evidence(process_id, window_handle).ok() != Some(before) {
        report.status = DisplayColorStatus::TargetChanged;
        report.outputs.clear();
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(value: &str) -> [u16; 32] {
        let mut result = [0; 32];
        for (slot, unit) in result.iter_mut().zip(value.encode_utf16()) {
            *slot = unit;
        }
        result
    }

    struct Reader {
        monitor_calls: u32,
        source_names: Vec<Result<[u16; 32], u32>>,
        colors: Vec<u32>,
        whites: Vec<u32>,
        move_monitor: bool,
        query_failure: bool,
        unsupported: bool,
    }

    impl DisplayColorReader for Reader {
        fn monitor(&mut self) -> Result<Monitor, Failure> {
            self.monitor_calls += 1;
            Ok(Monitor {
                name: name("DISPLAY1"),
                bounds: [
                    if self.move_monitor && self.monitor_calls == 2 {
                        1
                    } else {
                        0
                    },
                    0,
                    3840,
                    2400,
                ],
            })
        }
        fn active_paths(&mut self) -> Result<ActivePaths, Failure> {
            if self.query_failure {
                return Err(Failure(
                    DisplayColorStatus::DisplayConfigUnavailable,
                    Some(50),
                ));
            }
            Ok(ActivePaths {
                paths: (0..self.source_names.len())
                    .map(|index| Path {
                        source: DisplayConfigIdentity {
                            adapter_low: 1,
                            adapter_high: 0,
                            id: index as u32,
                        },
                        target: DisplayConfigIdentity {
                            adapter_low: 1,
                            adapter_high: 0,
                            id: index as u32 + 10,
                        },
                        target_available: true,
                    })
                    .collect(),
                mode_count: 2,
            })
        }
        fn source_name(&mut self, source: DisplayConfigIdentity) -> Result<[u16; 32], u32> {
            self.source_names[source.id as usize]
        }
        fn advanced_color(
            &mut self,
            target: DisplayConfigIdentity,
        ) -> Result<NativeAdvancedColorInfo, u32> {
            self.colors.push(target.id);
            if self.unsupported {
                Err(50)
            } else {
                Ok(NativeAdvancedColorInfo::decode(3, 0, 10))
            }
        }
        fn sdr_white_level(&mut self, target: DisplayConfigIdentity) -> Result<u32, u32> {
            self.whites.push(target.id);
            if self.unsupported { Err(87) } else { Ok(2000) }
        }
    }

    fn reader(names: Vec<Result<[u16; 32], u32>>) -> Reader {
        Reader {
            monitor_calls: 0,
            source_names: names,
            colors: Vec::new(),
            whites: Vec::new(),
            move_monitor: false,
            query_failure: false,
            unsupported: false,
        }
    }

    #[test]
    fn display_color_maps_only_exact_source_to_target_metadata() {
        let mut reader = reader(vec![Ok(name("DISPLAY10")), Ok(name("DISPLAY1"))]);
        let report = collect(&mut reader);
        assert_eq!(report.status, DisplayColorStatus::Collected);
        assert_eq!(reader.colors, [11]);
        assert_eq!(reader.whites, [11]);
        assert_eq!(report.outputs[0].sdr_white_level, Some(2000));
        assert_eq!(report.outputs[0].sdr_white_level_millinits, Some(160_000));
        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains("DISPLAY"));
        assert!(report.diagnostic_only);
        assert!(!report.configuration_stability_verified);
    }

    #[test]
    fn display_color_retains_all_cloned_targets() {
        let mut reader = reader(vec![Ok(name("DISPLAY1")), Ok(name("DISPLAY1"))]);
        let report = collect(&mut reader);
        assert_eq!(report.outputs.len(), 2);
        assert_eq!(reader.colors, [10, 11]);
    }

    #[test]
    fn display_color_unknown_queries_never_become_disabled_or_default_white() {
        let mut reader = reader(vec![Ok(name("DISPLAY1"))]);
        reader.unsupported = true;
        let report = collect(&mut reader);
        let output = &report.outputs[0];
        assert_eq!(output.advanced_color, None);
        assert_eq!(output.advanced_color_error, Some(50));
        assert_eq!(output.sdr_white_level, None);
        assert_eq!(output.sdr_white_level_millinits, None);
        assert_eq!(output.sdr_white_level_error, Some(87));
    }

    #[test]
    fn display_color_missing_source_does_not_query_unrelated_output() {
        let mut reader = reader(vec![Err(5), Ok(name("DISPLAY2"))]);
        let report = collect(&mut reader);
        assert_eq!(report.status, DisplayColorStatus::SourceMappingUnavailable);
        assert_eq!(report.source_query_errors, [(0, 5)]);
        assert!(!report.source_mapping_complete);
        assert!(reader.colors.is_empty());
    }

    #[test]
    fn display_color_changed_monitor_discards_target_metadata() {
        let mut reader = reader(vec![Ok(name("DISPLAY1"))]);
        reader.move_monitor = true;
        let report = collect(&mut reader);
        assert_eq!(report.status, DisplayColorStatus::TargetChanged);
        assert!(report.outputs.is_empty());
    }

    #[test]
    fn display_color_unavailable_config_remains_explicit() {
        let mut reader = reader(vec![Ok(name("DISPLAY1"))]);
        reader.query_failure = true;
        let report = collect(&mut reader);
        assert_eq!(report.status, DisplayColorStatus::DisplayConfigUnavailable);
        assert_eq!(report.os_error, Some(50));
        assert!(reader.colors.is_empty());
    }

    #[test]
    fn display_color_source_names_require_nonempty_terminated_exact_content() {
        assert!(!same_source_name(&[0; 32], &[0; 32]));
        assert!(!same_source_name(&[65; 32], &[65; 32]));
        assert!(!same_source_name(&name("DISPLAY1"), &name("DISPLAY10")));
        let mut trailing = name("DISPLAY1");
        trailing[31] = 65;
        assert!(same_source_name(&name("DISPLAY1"), &trailing));
    }

    #[test]
    fn display_color_counts_are_bounded_before_allocation() {
        assert!(bounded_counts(64, 128));
        assert!(!bounded_counts(0, 1));
        assert!(!bounded_counts(65, 1));
        assert!(!bounded_counts(1, 129));
    }

    #[test]
    fn display_color_decodes_native_flags_without_guessing_hdr_or_encoding() {
        let info = NativeAdvancedColorInfo::decode(0x8000_000d, 99, 16);
        assert!(info.advanced_color_supported);
        assert!(!info.advanced_color_enabled);
        assert!(info.wide_color_enforced);
        assert!(info.advanced_color_force_disabled);
        assert_eq!(info.raw_flags, 0x8000_000d);
        assert_eq!(info.color_encoding, 99);
        assert_eq!(info.bits_per_color_channel, 16);
    }
}
