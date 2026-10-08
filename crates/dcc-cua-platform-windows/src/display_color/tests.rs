use rstest::rstest;

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

#[rstest]
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

#[rstest]
fn display_color_retains_all_cloned_targets() {
    let mut reader = reader(vec![Ok(name("DISPLAY1")), Ok(name("DISPLAY1"))]);
    let report = collect(&mut reader);
    assert_eq!(report.outputs.len(), 2);
    assert_eq!(reader.colors, [10, 11]);
}

#[rstest]
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

#[rstest]
fn display_color_missing_source_does_not_query_unrelated_output() {
    let mut reader = reader(vec![Err(5), Ok(name("DISPLAY2"))]);
    let report = collect(&mut reader);
    assert_eq!(report.status, DisplayColorStatus::SourceMappingUnavailable);
    assert_eq!(report.source_query_errors, [(0, 5)]);
    assert!(!report.source_mapping_complete);
    assert!(reader.colors.is_empty());
}

#[rstest]
fn display_color_changed_monitor_discards_target_metadata() {
    let mut reader = reader(vec![Ok(name("DISPLAY1"))]);
    reader.move_monitor = true;
    let report = collect(&mut reader);
    assert_eq!(report.status, DisplayColorStatus::TargetChanged);
    assert!(report.outputs.is_empty());
}

#[rstest]
fn display_color_unavailable_config_remains_explicit() {
    let mut reader = reader(vec![Ok(name("DISPLAY1"))]);
    reader.query_failure = true;
    let report = collect(&mut reader);
    assert_eq!(report.status, DisplayColorStatus::DisplayConfigUnavailable);
    assert_eq!(report.os_error, Some(50));
    assert!(reader.colors.is_empty());
}

#[rstest]
fn display_color_source_names_require_nonempty_terminated_exact_content() {
    assert!(!same_source_name(&[0; 32], &[0; 32]));
    assert!(!same_source_name(&[65; 32], &[65; 32]));
    assert!(!same_source_name(&name("DISPLAY1"), &name("DISPLAY10")));
    let mut trailing = name("DISPLAY1");
    trailing[31] = 65;
    assert!(same_source_name(&name("DISPLAY1"), &trailing));
}

#[rstest]
fn display_color_counts_are_bounded_before_allocation() {
    assert!(bounded_counts(64, 128));
    assert!(!bounded_counts(0, 1));
    assert!(!bounded_counts(65, 1));
    assert!(!bounded_counts(1, 129));
}

#[rstest]
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
