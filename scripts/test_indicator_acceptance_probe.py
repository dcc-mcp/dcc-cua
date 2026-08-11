import importlib.util
import io
import json
import math
import sys
import unittest
from itertools import pairwise
from pathlib import Path

SCRIPT_PATH = Path(__file__).with_name("indicator-acceptance-probe.py")
SPEC = importlib.util.spec_from_file_location("indicator_acceptance_probe", SCRIPT_PATH)
assert SPEC is not None and SPEC.loader is not None
probe = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = probe
SPEC.loader.exec_module(probe)


class FrameContractTests(unittest.TestCase):
    def test_45_dip_scales_to_the_exact_monitor_dpi(self) -> None:
        contract = probe.FrameContract(
            thickness_dip=45, gradient_steps=20, alpha_max=132
        )

        self.assertEqual(contract.thickness_px(96), 45)
        self.assertEqual(contract.thickness_px(144), 68)
        self.assertEqual(contract.thickness_px(192), 90)

    def test_twenty_bands_cover_each_scaled_gradient_without_gaps(self) -> None:
        contract = probe.FrameContract(
            thickness_dip=45, gradient_steps=20, alpha_max=132
        )

        for dpi in (96, 144, 192):
            bands = contract.bands(dpi)
            self.assertEqual(len(bands), 20)
            self.assertEqual(bands[0].outer_inset_px, 0)
            self.assertEqual(bands[-1].inner_inset_px, contract.thickness_px(dpi))
            self.assertTrue(
                all(
                    left.inner_inset_px == right.outer_inset_px
                    for left, right in pairwise(bands)
                )
            )
            self.assertTrue(
                all(band.outer_inset_px < band.inner_inset_px for band in bands)
            )
            self.assertTrue(
                all(left.alpha >= right.alpha for left, right in pairwise(bands))
            )
            self.assertEqual(bands[0].alpha, 132)
            self.assertEqual(bands[-1].alpha, 0)

    def test_mutable_theme_cannot_redefine_the_fixed_acceptance_contract(self) -> None:
        class RegressedTheme:
            def read_text(self, *, encoding: str) -> str:
                self.encoding = encoding
                return json.dumps(
                    {
                        "indicator": {
                            "frame": {
                                "thickness_dip": 40,
                                "gradient_steps": 7,
                                "alpha_min": 1,
                                "alpha_max": 200,
                                "pulse_period_ms": 900,
                            }
                        }
                    }
                )

            def __str__(self) -> str:
                return "<regressed-theme>"

        with self.assertRaises(probe.ProbeFailure) as raised:
            probe._load_contract(RegressedTheme())

        self.assertEqual(
            raised.exception.reason["kind"], "theme_contract_acceptance_mismatch"
        )


class MotionContractTests(unittest.TestCase):
    def test_animated_samples_prove_range_change_and_1800ms_cycle(self) -> None:
        contract = probe.FrameContract(
            thickness_dip=45,
            gradient_steps=20,
            alpha_max=132,
            alpha_min=48,
            pulse_period_ms=1800,
        )
        samples = []
        for elapsed_ms in range(0, 1801, 100):
            wave = (math.cos(elapsed_ms / 1800 * math.tau) + 1.0) / 2.0
            samples.append(
                {
                    "elapsed_ms": elapsed_ms,
                    "outer_band_alpha": round(48 + (132 - 48) * wave),
                }
            )

        result = probe.evaluate_motion_samples(
            samples,
            contract,
            {
                "available": True,
                "resolved": "animate",
                "source": "fixture",
                "acceptance_authoritative": True,
            },
            interval_ms=100,
        )

        self.assertTrue(result["accepted"])
        self.assertEqual(result["contract"], "breathing_cycle")
        self.assertTrue(result["outer_alpha_changed"])
        self.assertTrue(result["range_covered_with_tolerance"])
        self.assertTrue(result["cycle_period_matched_with_tolerance"])

    def test_reduced_motion_is_typed_fixed_max_not_a_fake_cycle(self) -> None:
        contract = probe.FrameContract(
            thickness_dip=45, gradient_steps=20, alpha_max=132
        )

        result = probe.evaluate_motion_samples(
            [{"elapsed_ms": 0, "outer_band_alpha": 132}],
            contract,
            {
                "available": True,
                "resolved": "reduce",
                "source": "fixture",
                "acceptance_authoritative": True,
            },
            interval_ms=100,
        )

        self.assertTrue(result["accepted"])
        self.assertEqual(result["contract"], "fixed_max_reduced_motion")
        self.assertTrue(result["fixed_at_alpha_max"])
        self.assertFalse(result["outer_alpha_changed"])

    def test_constant_alpha_fails_closed_when_animation_is_expected(self) -> None:
        contract = probe.FrameContract(
            thickness_dip=45, gradient_steps=20, alpha_max=132
        )
        samples = [
            {"elapsed_ms": elapsed, "outer_band_alpha": 132}
            for elapsed in range(0, 1801, 100)
        ]

        result = probe.evaluate_motion_samples(
            samples,
            contract,
            {
                "available": True,
                "resolved": "animate",
                "source": "fixture",
                "acceptance_authoritative": True,
            },
            interval_ms=100,
        )

        self.assertFalse(result["accepted"])
        self.assertIn(
            "outer_band_alpha_did_not_change",
            {reason["kind"] for reason in result["blocking_reasons"]},
        )

    def test_triangular_wave_cannot_substitute_for_the_cosine_contract(self) -> None:
        contract = probe.FrameContract(
            thickness_dip=45,
            gradient_steps=20,
            alpha_max=132,
            alpha_min=48,
            pulse_period_ms=1800,
        )
        samples = []
        for elapsed_ms in range(0, 1801, 100):
            phase = elapsed_ms / contract.pulse_period_ms
            wave = 1 - 4 * phase if phase <= 0.5 else 4 * phase - 3
            samples.append(
                {
                    "elapsed_ms": elapsed_ms,
                    "outer_band_alpha": round(90 + 42 * wave),
                }
            )

        result = probe.evaluate_motion_samples(
            samples,
            contract,
            {
                "available": True,
                "resolved": "animate",
                "source": "fixture",
                "acceptance_authoritative": True,
            },
            interval_ms=100,
        )

        self.assertFalse(result["accepted"])
        self.assertIn(
            "breathing_cosine_shape_mismatch",
            {reason["kind"] for reason in result["blocking_reasons"]},
        )

    def test_sawtooth_wave_cannot_substitute_for_the_cosine_contract(self) -> None:
        contract = probe.FrameContract(
            thickness_dip=45,
            gradient_steps=20,
            alpha_max=132,
            alpha_min=48,
            pulse_period_ms=1800,
        )
        samples = []
        for elapsed_ms in range(0, 1801, 100):
            phase = elapsed_ms / contract.pulse_period_ms
            wave = 1.0 if elapsed_ms == contract.pulse_period_ms else 1 - 2 * phase
            samples.append(
                {
                    "elapsed_ms": elapsed_ms,
                    "outer_band_alpha": round(90 + 42 * wave),
                }
            )

        result = probe.evaluate_motion_samples(
            samples,
            contract,
            {
                "available": True,
                "resolved": "animate",
                "source": "fixture",
                "acceptance_authoritative": True,
            },
            interval_ms=100,
        )

        self.assertFalse(result["accepted"])
        self.assertIn(
            "breathing_cosine_shape_mismatch",
            {reason["kind"] for reason in result["blocking_reasons"]},
        )

    def test_cosine_fit_tolerates_33ms_read_timing_jitter(self) -> None:
        contract = probe.FrameContract(
            thickness_dip=45,
            gradient_steps=20,
            alpha_max=132,
            alpha_min=48,
            pulse_period_ms=1800,
        )
        samples = []
        phase_offset_ms = 273
        for index, elapsed_ms in enumerate(range(0, 1801, 100)):
            read_jitter_ms = 33 if index % 2 else -33
            phase = (
                elapsed_ms + phase_offset_ms + read_jitter_ms
            ) / contract.pulse_period_ms
            wave = (math.cos(phase * math.tau) + 1.0) / 2.0
            samples.append(
                {
                    "elapsed_ms": elapsed_ms,
                    "outer_band_alpha": round(48 + 84 * wave),
                }
            )

        result = probe.evaluate_motion_samples(
            samples,
            contract,
            {
                "available": True,
                "resolved": "animate",
                "source": "fixture",
                "acceptance_authoritative": True,
            },
            interval_ms=100,
        )

        self.assertTrue(result["accepted"])
        self.assertLessEqual(result["cosine_fit_max_alpha_error"], 6)

    def test_cli_motion_override_is_diagnostic_not_acceptance_authority(self) -> None:
        contract = probe.FrameContract(
            thickness_dip=45, gradient_steps=20, alpha_max=132
        )
        resolution = probe.resolve_motion(object(), "reduce")

        result = probe.evaluate_motion_samples(
            [{"elapsed_ms": 0, "outer_band_alpha": 132}],
            contract,
            resolution,
            interval_ms=100,
        )

        self.assertFalse(resolution["acceptance_authoritative"])
        self.assertFalse(result["accepted"])
        self.assertIn(
            "motion_resolution_not_authoritative",
            {reason["kind"] for reason in result["blocking_reasons"]},
        )


class MigrationContractTests(unittest.TestCase):
    def test_two_stable_monitor_visits_prove_mixed_dpi_negative_coordinate_migration(
        self,
    ) -> None:
        monitors = [monitor("0x1", 0, 96), monitor("0x2", -2560, 144)]
        samples = [
            {
                "converged": True,
                "monitor_handle": "0x1",
                "monitor_dpi": 96,
                "dwm_bounds": rect(0, 0, 1920, 1080),
            },
            {
                "converged": True,
                "monitor_handle": "0x1",
                "monitor_dpi": 96,
                "dwm_bounds": rect(0, 0, 1920, 1080),
            },
            {
                "converged": True,
                "monitor_handle": "0x2",
                "monitor_dpi": 144,
                "dwm_bounds": rect(-2560, 0, -640, 1080),
            },
            {
                "converged": True,
                "monitor_handle": "0x2",
                "monitor_dpi": 144,
                "dwm_bounds": rect(-2560, 0, -640, 1080),
            },
        ]

        result = probe.evaluate_migration_samples(
            samples, monitors, stable_samples_required=2
        )

        self.assertTrue(result["accepted"])
        self.assertEqual(result["stable_monitor_handles"], ["0x1", "0x2"])
        self.assertEqual(result["stable_effective_dpis"], [96, 144])
        self.assertTrue(result["negative_coordinate_visit"])

    def test_one_sample_monitor_transient_does_not_pass(self) -> None:
        monitors = [monitor("0x1", 0, 96), monitor("0x2", -2560, 144)]
        samples = [
            {
                "converged": True,
                "monitor_handle": "0x1",
                "monitor_dpi": 96,
                "dwm_bounds": rect(0, 0, 1920, 1080),
            },
            {
                "converged": True,
                "monitor_handle": "0x1",
                "monitor_dpi": 96,
                "dwm_bounds": rect(0, 0, 1920, 1080),
            },
            {
                "converged": True,
                "monitor_handle": "0x2",
                "monitor_dpi": 144,
                "dwm_bounds": rect(-2560, 0, -640, 1080),
            },
        ]

        result = probe.evaluate_migration_samples(
            samples, monitors, stable_samples_required=2
        )

        self.assertFalse(result["accepted"])
        self.assertIn(
            "stable_mixed_monitor_visits_required",
            {reason["kind"] for reason in result["blocking_reasons"]},
        )


def monitor(handle: str, left: int, dpi: int | None) -> dict[str, object]:
    effective_dpi: dict[str, object]
    if dpi is None:
        effective_dpi = {
            "available": False,
            "reason": {"kind": "effective_dpi_unavailable", "message": "fixture"},
        }
    else:
        effective_dpi = {
            "available": True,
            "x": dpi,
            "y": dpi,
            "source": "GetDpiForWindow(private_hidden_pmv2_probe)",
            "authority": "private_hidden_pmv2_probe",
        }
    bounds = {"left": left, "top": 0, "right": left + 1920, "bottom": 1080}
    return {
        "hmonitor": handle,
        "active": True,
        "bounds": bounds,
        "work_area": bounds,
        "effective_dpi": effective_dpi,
    }


def value(item: object) -> dict[str, object]:
    return {"available": True, "value": item}


def rect(left: int, top: int, right: int, bottom: int) -> dict[str, int]:
    return {"left": left, "top": top, "right": right, "bottom": bottom}


def target_snapshot(
    pid: int,
    hwnd: str,
    dpi: int = 144,
    monitor_handle: str = "0x2",
    bounds: dict[str, int] | None = None,
    monitor_bounds: dict[str, int] | None = None,
    monitor_work_area: dict[str, int] | None = None,
) -> dict[str, object]:
    if bounds is None:
        bounds = rect(-2560, 0, 0, 1440)
    if monitor_bounds is None:
        monitor_bounds = bounds
    if monitor_work_area is None:
        monitor_work_area = monitor_bounds
    return {
        "requested_pid": pid,
        "requested_hwnd": hwnd,
        "is_window": value(True),
        "process_id": value(pid),
        "window_rect": value(bounds),
        "dwm_extended_frame_bounds": value(bounds),
        "window_dpi": value(dpi),
        "monitor_handle": value(monitor_handle),
        "monitor_bounds": value(monitor_bounds),
        "monitor_work_area": value(monitor_work_area),
        "monitor_effective_dpi": {
            "available": True,
            "value": dpi,
            "source": "GetDpiForWindow(private_hidden_pmv2_probe)",
            "authority": "private_hidden_pmv2_probe",
        },
    }


def overlay_window(
    hwnd: str,
    class_name: str,
    *,
    dpi: int = 144,
    window_rect: dict[str, int] | None = None,
    region_bounds: dict[str, int] | None = None,
    region_band: tuple[int, int] | None = None,
    alpha: int = 132,
    visible: bool = True,
    rounded: bool = False,
    monitor_handle: str = "0x2",
    process_id: int = 9001,
    thread_id: int = 9002,
    topmost: bool = False,
    no_activate: bool = True,
    tool_window: bool = True,
    transparent: bool = True,
    owner_hwnd: str | None = None,
) -> dict[str, object]:
    if window_rect is None:
        window_rect = rect(-2560, 0, 0, 1440)
    region = (
        {
            "available": True,
            "complexity": "complex",
            "bounds": region_bounds,
            "edge_bands": {
                edge: (
                    {
                        "available": True,
                        "outer_inset_px": region_band[0],
                        "inner_inset_px": region_band[1],
                        "source": f"PtInRegion({edge}_center_scan)",
                    }
                    if region_band is not None
                    else {
                        "available": False,
                        "reason": {
                            "kind": "fixture_unavailable",
                            "message": "fixture",
                        },
                    }
                )
                for edge in ("top", "bottom", "left", "right")
            },
            "point_membership": {
                "top_left_corner": not rounded,
                "top_right_corner": not rounded,
                "bottom_left_corner": not rounded,
                "bottom_right_corner": not rounded,
                "top_midpoint": True,
                "bottom_midpoint": True,
                "left_midpoint": True,
                "right_midpoint": True,
                "center": True,
            },
        }
        if region_bounds is not None
        else {
            "available": True,
            "complexity": "null",
            "bounds": None,
            "edge_bands": {
                edge: {
                    "available": False,
                    "reason": {"kind": "null_region", "message": "fixture"},
                }
                for edge in ("top", "bottom", "left", "right")
            },
            "point_membership": None,
        }
    )
    return {
        "hwnd": hwnd,
        "class_name": class_name,
        "process_id": value(process_id),
        "thread_id": value(thread_id),
        "visible": value(visible),
        "window_rect": value(window_rect),
        "dwm_extended_frame_bounds": value(window_rect),
        "dpi": value(dpi),
        "monitor_handle": value(monitor_handle),
        "region": region,
        "layered": {
            "available": True,
            "value": True,
            "alpha": value(alpha),
            "flags": value(2),
        },
        "display_affinity": value(17),
        "extended_styles": value(
            {
                "topmost": topmost,
                "no_activate": no_activate,
                "tool_window": tool_window,
                "transparent": transparent,
            }
        ),
        "owner_hwnd": value(owner_hwnd),
        "z_order": {
            "available": True,
            "relation_to_target": "above",
            "desktop_index_top_to_bottom": 1,
        },
    }


def converged_overlays(
    contract: object,
    dpi: int = 144,
    target_bounds: dict[str, int] | None = None,
) -> list[dict[str, object]]:
    if target_bounds is None:
        target_bounds = rect(-2560, 0, 0, 1440)
    frames = []
    width = target_bounds["right"] - target_bounds["left"]
    height = target_bounds["bottom"] - target_bounds["top"]
    monitor_handle = "0x2" if target_bounds["left"] < 0 else "0x1"
    for band in contract.bands(dpi):
        inset = band.outer_inset_px
        frames.append(
            overlay_window(
                f"0x{1000 + band.index:x}",
                "DccCuaControlFrame",
                dpi=dpi,
                window_rect=target_bounds,
                region_bounds=rect(inset, inset, width - inset, height - inset),
                region_band=(band.outer_inset_px, band.inner_inset_px),
                alpha=band.alpha,
                monitor_handle=monitor_handle,
            )
        )
    scale = lambda dip: (dip * dpi + 48) // 96
    banner_height = scale(44)
    banner_width = min(scale(480), max(1, width - scale(16)))
    banner_left = target_bounds["left"] + (width - banner_width) // 2
    banner_top = target_bounds["top"] + scale(16)
    banner = overlay_window(
        "0x2000",
        "DccCuaControlBanner",
        dpi=dpi,
        window_rect=rect(
            banner_left,
            banner_top,
            banner_left + banner_width,
            banner_top + banner_height,
        ),
        region_bounds=rect(0, 0, banner_width, banner_height),
        alpha=248,
        rounded=True,
        monitor_handle=monitor_handle,
    )
    dpi_probe = overlay_window(
        "0x3000",
        "DccCuaControlFrame",
        dpi=dpi,
        window_rect=rect(
            target_bounds["left"] + width // 2,
            target_bounds["top"] + height // 2,
            target_bounds["left"] + width // 2 + 1,
            target_bounds["top"] + height // 2 + 1,
        ),
        alpha=0,
        visible=False,
        monitor_handle=monitor_handle,
    )
    visible_overlays = [banner, *frames]
    target_index = len(visible_overlays)
    for index, window in enumerate(visible_overlays):
        next_hwnd = (
            visible_overlays[index + 1]["hwnd"]
            if index + 1 < len(visible_overlays)
            else "0x123"
        )
        window["z_order"] = {
            "available": True,
            "relation_to_target": "above",
            "desktop_index_top_to_bottom": index,
            "target_index_top_to_bottom": target_index,
            "next_hwnd": next_hwnd,
        }
    return [banner, *frames, dpi_probe]


class TopologyGateTests(unittest.TestCase):
    def setUp(self) -> None:
        self.contract = probe.FrameContract(
            thickness_dip=45, gradient_steps=20, alpha_max=132
        )

    def test_gate_fails_closed_with_a_typed_reason(self) -> None:
        cases = (
            ([monitor("0x1", 0, 96)], "insufficient_active_monitors"),
            (
                [monitor("0x1", 0, 96), monitor("0x2", 1920, 144)],
                "negative_coordinate_monitor_required",
            ),
            (
                [monitor("0x1", 0, 96), monitor("0x2", -1920, 96)],
                "mixed_effective_dpi_required",
            ),
            (
                [monitor("0x1", 0, 96), monitor("0x2", -1920, None)],
                "monitor_effective_dpi_unavailable",
            ),
        )

        for monitors, expected_kind in cases:
            with self.subTest(expected_kind=expected_kind):
                summary = probe.evaluate_topology(monitors, self.contract)
                self.assertFalse(summary["eligible"])
                self.assertIn(
                    expected_kind,
                    {reason["kind"] for reason in summary["blocking_reasons"]},
                )

    def test_gate_accepts_negative_coordinate_mixed_dpi_topology(self) -> None:
        summary = probe.evaluate_topology(
            [
                monitor("0x1", 0, 96),
                monitor("0x2", -2560, 144),
                monitor("0x3", 1920, 192),
            ],
            self.contract,
        )

        self.assertTrue(summary["eligible"])
        self.assertEqual(summary["blocking_reasons"], [])
        expected = {
            item["dpi"]: item["thickness_px"]
            for item in summary["expected_frame_contracts"]
        }
        self.assertEqual(expected, {96: 45, 144: 68, 192: 90})

    def test_gate_rejects_legacy_monitor_dpi_as_non_authoritative(self) -> None:
        monitors = [monitor("0x1", 0, 96), monitor("0x2", -2560, 144)]
        monitors[1]["effective_dpi"]["source"] = "GetDpiForMonitor(MDT_EFFECTIVE_DPI)"
        monitors[1]["effective_dpi"]["authority"] = "api_reported_effective_dpi"

        summary = probe.evaluate_topology(monitors, self.contract)

        self.assertFalse(summary["eligible"])
        self.assertIn(
            "monitor_effective_dpi_authority_untrusted",
            {reason["kind"] for reason in summary["blocking_reasons"]},
        )

    def test_private_probe_must_land_on_the_intended_hmonitor(self) -> None:
        matched = probe.evaluate_private_probe_binding("0x2", "0x2")
        mismatched = probe.evaluate_private_probe_binding("0x2", "0x1")

        self.assertTrue(matched["available"])
        self.assertFalse(mismatched["available"])
        self.assertEqual(
            mismatched["reason"]["kind"], "private_pmv2_probe_monitor_mismatch"
        )


class SampleEvaluationTests(unittest.TestCase):
    def test_structural_evidence_converges_without_fabricating_probe_role(self) -> None:
        contract = probe.FrameContract(
            thickness_dip=45, gradient_steps=20, alpha_max=132
        )
        target = target_snapshot(42, "0x123")

        result = probe.evaluate_sample(target, converged_overlays(contract), contract)

        self.assertTrue(result["converged"])
        self.assertEqual(result["frame_contract"]["observed_frame_count"], 20)
        self.assertTrue(result["frame_contract"]["bands_cover_without_gaps"])
        self.assertTrue(result["frame_contract"]["alpha_monotonic_inward"])
        self.assertEqual(result["dpi_probe_evidence"]["candidate_hwnds"], ["0x3000"])
        self.assertFalse(result["dpi_probe_evidence"]["logical_role"]["available"])
        self.assertEqual(
            result["dpi_probe_evidence"]["logical_role"]["reason"]["kind"],
            "logical_role_not_exposed_by_win32",
        )

    def test_frame_matching_uses_unvirtualized_dwm_bounds(self) -> None:
        contract = probe.FrameContract(
            thickness_dip=45, gradient_steps=20, alpha_max=132
        )
        target = target_snapshot(42, "0x123")
        windows = converged_overlays(contract)
        for window in windows:
            if (
                window["class_name"] == "DccCuaControlFrame"
                and window["region"]["complexity"] != "null"
            ):
                window["window_rect"] = value(rect(-1707, 0, 0, 960))

        result = probe.evaluate_sample(target, windows, contract)

        self.assertTrue(result["converged"])
        self.assertEqual(
            result["frame_contract"]["geometry_source"],
            "DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)",
        )

    def test_region_outer_box_without_inner_cutout_does_not_pass(self) -> None:
        contract = probe.FrameContract(
            thickness_dip=45, gradient_steps=20, alpha_max=132
        )
        target = target_snapshot(42, "0x123")
        windows = converged_overlays(contract)
        first_frame = next(
            window
            for window in windows
            if window["class_name"] == "DccCuaControlFrame"
            and window["region"]["complexity"] == "complex"
        )
        first_frame["region"]["edge_bands"]["top"] = {
            "available": False,
            "reason": {
                "kind": "inner_cutout_not_observed",
                "message": "solid region never exits before its center",
            },
        }

        result = probe.evaluate_sample(target, windows, contract)

        self.assertFalse(result["converged"])
        self.assertIn(
            "gradient_regions_do_not_cover_contract",
            {reason["kind"] for reason in result["blocking_reasons"]},
        )

    def test_top_only_band_cannot_substitute_for_four_sided_ring(self) -> None:
        contract = probe.FrameContract(
            thickness_dip=45, gradient_steps=20, alpha_max=132
        )
        target = target_snapshot(42, "0x123")
        windows = converged_overlays(contract)
        first_frame = next(
            window
            for window in windows
            if window["class_name"] == "DccCuaControlFrame"
            and window["region"]["complexity"] == "complex"
        )
        first_frame["region"]["edge_bands"]["right"] = {
            "available": False,
            "reason": {
                "kind": "right_edge_band_unavailable",
                "message": "top-only region fixture",
            },
        }

        result = probe.evaluate_sample(target, windows, contract)

        self.assertFalse(result["converged"])
        self.assertFalse(result["frame_contract"]["four_sided_rings"])

    def test_sample_rejects_untrusted_target_monitor_dpi(self) -> None:
        contract = probe.FrameContract(
            thickness_dip=45, gradient_steps=20, alpha_max=132
        )
        target = target_snapshot(42, "0x123")
        target["monitor_effective_dpi"]["authority"] = "legacy_monitor_api"

        result = probe.evaluate_sample(target, converged_overlays(contract), contract)

        self.assertFalse(result["converged"])
        self.assertIn(
            "target_monitor_effective_dpi_authority_untrusted",
            {reason["kind"] for reason in result["blocking_reasons"]},
        )

    def test_flat_alpha_slab_cannot_substitute_for_the_inward_fade(self) -> None:
        contract = probe.FrameContract(
            thickness_dip=45, gradient_steps=20, alpha_max=132
        )
        target = target_snapshot(42, "0x123")
        windows = converged_overlays(contract)
        for window in windows:
            if (
                window["class_name"] == "DccCuaControlFrame"
                and window["region"]["complexity"] != "null"
            ):
                window["layered"]["alpha"] = value(132)

        result = probe.evaluate_sample(target, windows, contract)

        self.assertFalse(result["converged"])
        self.assertIn(
            "gradient_alpha_profile_mismatch",
            {reason["kind"] for reason in result["blocking_reasons"]},
        )

    def test_alpha_readback_requires_lwa_alpha_flag(self) -> None:
        contract = probe.FrameContract(
            thickness_dip=45, gradient_steps=20, alpha_max=132
        )
        target = target_snapshot(42, "0x123")
        windows = converged_overlays(contract)
        first_frame = next(
            window
            for window in windows
            if window["class_name"] == "DccCuaControlFrame"
            and window["region"]["complexity"] == "complex"
        )
        first_frame["layered"]["flags"] = value(1)

        result = probe.evaluate_sample(target, windows, contract)

        self.assertFalse(result["converged"])
        self.assertIn(
            "layered_alpha_flag_missing_or_unavailable",
            {reason["kind"] for reason in result["blocking_reasons"]},
        )

    def test_banner_must_match_the_target_scoped_dip_geometry(self) -> None:
        contract = probe.FrameContract(
            thickness_dip=45, gradient_steps=20, alpha_max=132
        )
        target = target_snapshot(42, "0x123")
        windows = converged_overlays(contract)
        banner = next(
            window
            for window in windows
            if window["class_name"] == "DccCuaControlBanner"
        )
        off_target = rect(10_000, 10_000, 10_720, 10_066)
        banner["window_rect"] = value(off_target)
        banner["dwm_extended_frame_bounds"] = value(off_target)

        result = probe.evaluate_sample(target, windows, contract)

        self.assertFalse(result["converged"])
        self.assertIn(
            "banner_geometry_mismatch_or_unavailable",
            {reason["kind"] for reason in result["blocking_reasons"]},
        )

    def test_banner_requires_a_rounded_region_not_a_solid_rectangle(self) -> None:
        contract = probe.FrameContract(
            thickness_dip=45, gradient_steps=20, alpha_max=132
        )
        target = target_snapshot(42, "0x123")
        windows = converged_overlays(contract)
        banner = next(
            window
            for window in windows
            if window["class_name"] == "DccCuaControlBanner"
        )
        for corner in (
            "top_left_corner",
            "top_right_corner",
            "bottom_left_corner",
            "bottom_right_corner",
        ):
            banner["region"]["point_membership"][corner] = True

        result = probe.evaluate_sample(target, windows, contract)

        self.assertFalse(result["converged"])
        self.assertIn(
            "banner_rounded_region_unproven",
            {reason["kind"] for reason in result["blocking_reasons"]},
        )

    def test_indicator_family_requires_one_coherent_process_and_thread(self) -> None:
        contract = probe.FrameContract(
            thickness_dip=45, gradient_steps=20, alpha_max=132
        )
        target = target_snapshot(42, "0x123")
        cases = (
            ("process_id", value(9003), "indicator_family_process_mismatch"),
            ("thread_id", value(9004), "indicator_family_thread_mismatch"),
        )

        for field, replacement, expected_reason in cases:
            with self.subTest(field=field):
                windows = converged_overlays(contract)
                windows[1][field] = replacement

                result = probe.evaluate_sample(target, windows, contract)

                self.assertFalse(result["converged"])
                self.assertIn(
                    expected_reason,
                    {reason["kind"] for reason in result["blocking_reasons"]},
                )

    def test_indicator_family_rejects_unsafe_styles_and_non_null_owner(self) -> None:
        contract = probe.FrameContract(
            thickness_dip=45, gradient_steps=20, alpha_max=132
        )
        target = target_snapshot(42, "0x123")
        cases = (
            ("topmost", True, "indicator_global_topmost_forbidden"),
            ("no_activate", False, "indicator_noactivate_style_required"),
            ("tool_window", False, "indicator_toolwindow_style_required"),
            ("transparent", False, "indicator_transparent_style_required"),
        )

        for field, replacement, expected_reason in cases:
            with self.subTest(field=field):
                windows = converged_overlays(contract)
                frame = windows[1]
                frame["extended_styles"]["value"][field] = replacement

                result = probe.evaluate_sample(target, windows, contract)

                self.assertFalse(result["converged"])
                self.assertIn(
                    expected_reason,
                    {reason["kind"] for reason in result["blocking_reasons"]},
                )

        windows = converged_overlays(contract)
        windows[1]["owner_hwnd"] = value("0x123")
        result = probe.evaluate_sample(target, windows, contract)
        self.assertFalse(result["converged"])
        self.assertIn(
            "indicator_owner_must_be_null",
            {reason["kind"] for reason in result["blocking_reasons"]},
        )

    def test_indicator_family_must_bind_to_the_target_monitor(self) -> None:
        contract = probe.FrameContract(
            thickness_dip=45, gradient_steps=20, alpha_max=132
        )
        target = target_snapshot(42, "0x123")
        windows = converged_overlays(contract)
        windows[0]["monitor_handle"] = value("0x1")

        result = probe.evaluate_sample(target, windows, contract)

        self.assertFalse(result["converged"])
        self.assertIn(
            "indicator_monitor_mismatch_or_unavailable",
            {reason["kind"] for reason in result["blocking_reasons"]},
        )

    def test_unclassified_frame_class_window_fails_closed(self) -> None:
        contract = probe.FrameContract(
            thickness_dip=45, gradient_steps=20, alpha_max=132
        )
        target = target_snapshot(42, "0x123")
        windows = converged_overlays(contract)
        windows.append(
            overlay_window(
                "0x4000",
                "DccCuaControlFrame",
                window_rect=rect(-100, -100, -90, -90),
            )
        )

        result = probe.evaluate_sample(target, windows, contract)

        self.assertFalse(result["converged"])
        self.assertIn(
            "unclassified_indicator_frame_windows",
            {reason["kind"] for reason in result["blocking_reasons"]},
        )

    def test_visible_indicator_family_must_be_one_contiguous_target_scoped_block(
        self,
    ) -> None:
        contract = probe.FrameContract(
            thickness_dip=45, gradient_steps=20, alpha_max=132
        )
        target = target_snapshot(42, "0x123")
        windows = converged_overlays(contract)
        windows[5]["z_order"]["next_hwnd"] = "0xdead"

        result = probe.evaluate_sample(target, windows, contract)

        self.assertFalse(result["converged"])
        self.assertIn(
            "target_scoped_z_order_block_unproven",
            {reason["kind"] for reason in result["blocking_reasons"]},
        )

    def test_hidden_dpi_probe_z_order_is_not_part_of_the_visible_target_block(
        self,
    ) -> None:
        contract = probe.FrameContract(
            thickness_dip=45, gradient_steps=20, alpha_max=132
        )
        target = target_snapshot(42, "0x123")
        windows = converged_overlays(contract)
        hidden_probe = windows[-1]
        hidden_probe["z_order"] = {
            "available": True,
            "relation_to_target": "below",
            "desktop_index_top_to_bottom": 22,
            "target_index_top_to_bottom": 21,
            "next_hwnd": None,
        }

        result = probe.evaluate_sample(target, windows, contract)

        self.assertTrue(result["converged"])

    def test_owned_modal_may_remain_above_the_target_scoped_overlay_block(self) -> None:
        contract = probe.FrameContract(
            thickness_dip=45, gradient_steps=20, alpha_max=132
        )
        target = target_snapshot(42, "0x123")
        windows = converged_overlays(contract)
        for window in windows[:-1]:
            window["z_order"]["desktop_index_top_to_bottom"] += 1
            window["z_order"]["target_index_top_to_bottom"] += 1

        result = probe.evaluate_sample(target, windows, contract)

        self.assertTrue(result["converged"])
        self.assertTrue(result["indicator_family"]["target_scoped_z_order_block"])


class FakeObservationSource:
    def __init__(
        self, monitors: list[dict[str, object]], snapshots: list[dict[str, object]]
    ) -> None:
        self._monitors = monitors
        self._snapshots = iter(snapshots)
        self.observed_targets: list[tuple[int, int]] = []

    def enumerate_monitors(self) -> list[dict[str, object]]:
        return self._monitors

    def observe(self, target_pid: int, target_hwnd: int) -> dict[str, object]:
        self.observed_targets.append((target_pid, target_hwnd))
        return next(self._snapshots)

    def system_animation_enabled(self) -> dict[str, object]:
        return {"available": True, "value": False, "source": "fixture"}


class FakeClock:
    def __init__(self) -> None:
        self.value = 0.0
        self.sleeps: list[float] = []

    def __call__(self) -> float:
        return self.value

    def sleep(self, seconds: float) -> None:
        self.sleeps.append(round(seconds, 10))
        self.value = round(self.value + seconds, 10)


class ProbeRunTests(unittest.TestCase):
    def test_jsonl_events_capture_convergence_and_cleanup(self) -> None:
        contract = probe.FrameContract(
            thickness_dip=45, gradient_steps=20, alpha_max=132
        )
        positive_bounds = rect(0, 0, 1920, 1080)
        negative_bounds = rect(-2560, 0, -640, 1080)
        positive_target = target_snapshot(42, "0x123", 96, "0x1", positive_bounds)
        negative_target = target_snapshot(42, "0x123", 144, "0x2", negative_bounds)
        source = FakeObservationSource(
            [monitor("0x1", 0, 96), monitor("0x2", -2560, 144)],
            [
                {
                    "target": positive_target,
                    "indicator_windows": converged_overlays(
                        contract, 96, positive_bounds
                    ),
                },
                {
                    "target": positive_target,
                    "indicator_windows": converged_overlays(
                        contract, 96, positive_bounds
                    ),
                },
                {
                    "target": negative_target,
                    "indicator_windows": converged_overlays(
                        contract, 144, negative_bounds
                    ),
                },
                {
                    "target": negative_target,
                    "indicator_windows": converged_overlays(
                        contract, 144, negative_bounds
                    ),
                },
                {"target": negative_target, "indicator_windows": []},
            ],
        )
        emitted: list[dict[str, object]] = []
        clock = FakeClock()
        config = probe.ProbeConfig(
            target_pid=42,
            target_hwnd=0x123,
            interval_ms=100,
            duration_ms=400,
            require_cleanup=True,
        )

        result = probe.run_probe(
            source,
            config,
            contract,
            emitted.append,
            sleep=clock.sleep,
            timestamp=lambda: "2026-08-11T00:00:00Z",
            monotonic=clock,
        )

        self.assertEqual(result.exit_code, 0)
        self.assertTrue(result.converged)
        self.assertEqual(result.cleanup_status, "cleaned")
        self.assertEqual(
            [event["event"] for event in emitted],
            [
                "topology_summary",
                "sample",
                "sample",
                "sample",
                "sample",
                "sample",
                "motion_summary",
                "migration_summary",
                "cleanup_summary",
            ],
        )
        self.assertTrue(emitted[1]["evaluation"]["converged"])
        self.assertFalse(emitted[5]["evaluation"]["converged"])
        self.assertEqual(emitted[-3]["contract"], "fixed_max_reduced_motion")
        self.assertTrue(emitted[-3]["accepted"])
        self.assertTrue(emitted[-2]["accepted"])
        self.assertEqual(emitted[-1]["ever_seen_count"], 22)
        self.assertEqual(emitted[-1]["remaining_hwnds"], [])
        self.assertEqual(source.observed_targets, [(42, 0x123)] * 5)
        self.assertEqual(clock.sleeps, [0.1, 0.1, 0.1, 0.1])

    def test_steady_state_requires_the_final_sample_to_remain_converged(self) -> None:
        contract = probe.FrameContract(
            thickness_dip=45, gradient_steps=20, alpha_max=132
        )
        positive_bounds = rect(0, 0, 1920, 1080)
        negative_bounds = rect(-2560, 0, -640, 1080)
        positive_target = target_snapshot(42, "0x123", 96, "0x1", positive_bounds)
        negative_target = target_snapshot(42, "0x123", 144, "0x2", negative_bounds)
        source = FakeObservationSource(
            [monitor("0x1", 0, 96), monitor("0x2", -2560, 144)],
            [
                {
                    "target": positive_target,
                    "indicator_windows": converged_overlays(
                        contract, 96, positive_bounds
                    ),
                },
                {
                    "target": positive_target,
                    "indicator_windows": converged_overlays(
                        contract, 96, positive_bounds
                    ),
                },
                {
                    "target": negative_target,
                    "indicator_windows": converged_overlays(
                        contract, 144, negative_bounds
                    ),
                },
                {
                    "target": negative_target,
                    "indicator_windows": converged_overlays(
                        contract, 144, negative_bounds
                    ),
                },
                {"target": negative_target, "indicator_windows": []},
            ],
        )
        emitted: list[dict[str, object]] = []
        clock = FakeClock()

        result = probe.run_probe(
            source,
            probe.ProbeConfig(42, 0x123, 100, 400, False),
            contract,
            emitted.append,
            sleep=clock.sleep,
            timestamp=lambda: "2026-08-11T00:00:00Z",
            monotonic=clock,
        )

        self.assertEqual(result.exit_code, 4)
        self.assertFalse(emitted[-1]["accepted"])
        self.assertIn(
            "steady_state_final_sample_not_converged",
            {reason["kind"] for reason in emitted[-1]["blocking_reasons"]},
        )

    def test_cleanup_rejects_partial_teardown_before_all_windows_disappear(
        self,
    ) -> None:
        contract = probe.FrameContract(
            thickness_dip=45, gradient_steps=20, alpha_max=132
        )
        positive_bounds = rect(0, 0, 1920, 1080)
        negative_bounds = rect(-2560, 0, -640, 1080)
        positive_target = target_snapshot(42, "0x123", 96, "0x1", positive_bounds)
        negative_target = target_snapshot(42, "0x123", 144, "0x2", negative_bounds)
        partial = converged_overlays(contract, 144, negative_bounds)
        partial.pop(10)
        source = FakeObservationSource(
            [monitor("0x1", 0, 96), monitor("0x2", -2560, 144)],
            [
                {
                    "target": positive_target,
                    "indicator_windows": converged_overlays(
                        contract, 96, positive_bounds
                    ),
                },
                {
                    "target": positive_target,
                    "indicator_windows": converged_overlays(
                        contract, 96, positive_bounds
                    ),
                },
                {
                    "target": negative_target,
                    "indicator_windows": converged_overlays(
                        contract, 144, negative_bounds
                    ),
                },
                {
                    "target": negative_target,
                    "indicator_windows": converged_overlays(
                        contract, 144, negative_bounds
                    ),
                },
                {"target": negative_target, "indicator_windows": partial},
                {"target": negative_target, "indicator_windows": []},
            ],
        )
        emitted: list[dict[str, object]] = []
        clock = FakeClock()

        result = probe.run_probe(
            source,
            probe.ProbeConfig(42, 0x123, 100, 500, True),
            contract,
            emitted.append,
            sleep=clock.sleep,
            timestamp=lambda: "2026-08-11T00:00:00Z",
            monotonic=clock,
        )

        self.assertEqual(result.exit_code, 4)
        self.assertFalse(emitted[-1]["accepted"])
        self.assertIn(
            "partial_indicator_teardown_observed",
            {reason["kind"] for reason in emitted[-1]["blocking_reasons"]},
        )

    def test_cli_emits_machine_readable_jsonl(self) -> None:
        contract = probe.FrameContract(
            thickness_dip=45, gradient_steps=20, alpha_max=132
        )
        positive_bounds = rect(0, 0, 1920, 1080)
        negative_bounds = rect(-2560, 0, -640, 1080)
        positive_target = target_snapshot(42, "0x123", 96, "0x1", positive_bounds)
        negative_target = target_snapshot(42, "0x123", 144, "0x2", negative_bounds)
        source = FakeObservationSource(
            [monitor("0x1", 0, 96), monitor("0x2", -2560, 144)],
            [
                {
                    "target": positive_target,
                    "indicator_windows": converged_overlays(
                        contract, 96, positive_bounds
                    ),
                },
                {
                    "target": positive_target,
                    "indicator_windows": converged_overlays(
                        contract, 96, positive_bounds
                    ),
                },
                {
                    "target": negative_target,
                    "indicator_windows": converged_overlays(
                        contract, 144, negative_bounds
                    ),
                },
                {
                    "target": negative_target,
                    "indicator_windows": converged_overlays(
                        contract, 144, negative_bounds
                    ),
                },
            ],
        )
        stdout = io.StringIO()
        clock = FakeClock()

        exit_code = probe.main(
            ["--target-pid", "42", "--target-hwnd", "0x123", "--duration-ms", "300"],
            source=source,
            stdout=stdout,
            sleep=clock.sleep,
            timestamp=lambda: "2026-08-11T00:00:00Z",
            monotonic=clock,
        )

        events = [json.loads(line) for line in stdout.getvalue().splitlines()]
        self.assertEqual(exit_code, 0)
        self.assertEqual(events[0]["event"], "topology_summary")
        self.assertTrue(events[0]["observation_only"])
        self.assertEqual(events[1]["target"]["requested_hwnd"], "0x123")
        self.assertEqual(events[-2]["event"], "migration_summary")
        self.assertEqual(events[-1]["event"], "cleanup_summary")


if __name__ == "__main__":
    unittest.main()
