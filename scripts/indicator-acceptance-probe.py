#!/usr/bin/env python3
"""Read-only acceptance probe for the native DCC CUA control indicator."""

from __future__ import annotations

import argparse
import ctypes
import json
import math
import sys
import time
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from datetime import UTC, datetime
from itertools import pairwise
from pathlib import Path
from typing import Any, ClassVar, Protocol, TextIO

SCHEMA = "dcc-cua.indicator-acceptance.v1"


@dataclass(frozen=True)
class FrameBand:
    index: int
    outer_inset_px: int
    inner_inset_px: int
    alpha: int


@dataclass(frozen=True)
class FrameContract:
    """Device-independent target-frame geometry and opacity contract."""

    thickness_dip: int
    gradient_steps: int
    alpha_max: int
    alpha_min: int = 48
    pulse_period_ms: int = 1_800

    def thickness_px(self, dpi: int) -> int:
        """Match the indicator's integer half-up DIP scaling."""

        return (self.thickness_dip * dpi + 48) // 96

    def bands(self, dpi: int) -> tuple[FrameBand, ...]:
        thickness = self.thickness_px(dpi)
        divisor = self.gradient_steps * self.gradient_steps
        return tuple(
            FrameBand(
                index=index,
                outer_inset_px=thickness * index // self.gradient_steps,
                inner_inset_px=thickness * (index + 1) // self.gradient_steps,
                alpha=(
                    self.alpha_max
                    * (self.gradient_steps - index)
                    * (self.gradient_steps - index)
                    // divisor
                ),
            )
            for index in range(self.gradient_steps)
        )


# This is the acceptance requirement, not a value that the implementation theme may
# redefine. The mutable theme is checked against it before any observation is scored.
ACCEPTANCE_FRAME_CONTRACT = FrameContract(
    thickness_dip=45,
    gradient_steps=20,
    alpha_min=48,
    alpha_max=132,
    pulse_period_ms=1_800,
)


def _fit_cosine_alpha(
    points: Sequence[tuple[int, int]], contract: FrameContract
) -> dict[str, Any]:
    """Fit an unknown phase while keeping period, midpoint, and amplitude fixed."""

    if not points:
        return {
            "available": False,
            "phase_ms": None,
            "max_alpha_error": None,
            "rms_alpha_error": None,
        }
    midpoint = (contract.alpha_min + contract.alpha_max) / 2.0
    amplitude = (contract.alpha_max - contract.alpha_min) / 2.0
    best: tuple[float, float, int] | None = None
    for phase_ms in range(contract.pulse_period_ms):
        errors = [
            abs(
                alpha
                - (
                    midpoint
                    + amplitude
                    * math.cos(
                        math.tau * (elapsed_ms + phase_ms) / contract.pulse_period_ms
                    )
                )
            )
            for elapsed_ms, alpha in points
        ]
        candidate = (
            max(errors),
            math.sqrt(sum(error * error for error in errors) / len(errors)),
            phase_ms,
        )
        if best is None or candidate < best:
            best = candidate
    assert best is not None
    return {
        "available": True,
        "phase_ms": best[2],
        "max_alpha_error": best[0],
        "rms_alpha_error": best[1],
    }


def evaluate_motion_samples(
    samples: Sequence[Mapping[str, Any]],
    contract: FrameContract,
    resolution: Mapping[str, Any],
    *,
    interval_ms: int,
) -> dict[str, Any]:
    """Aggregate temporal alpha evidence without inferring inaccessible session state."""

    reasons: list[dict[str, Any]] = []
    if resolution.get("acceptance_authoritative") is not True:
        reasons.append(
            _reason(
                "motion_resolution_not_authoritative",
                "motion mode was not resolved from the read-only OS animation preference",
                resolution=dict(resolution),
            )
        )
    points = [
        (int(sample["elapsed_ms"]), int(sample["outer_band_alpha"]))
        for sample in samples
        if isinstance(sample.get("elapsed_ms"), int)
        and isinstance(sample.get("outer_band_alpha"), int)
    ]
    alphas = [alpha for _elapsed, alpha in points]
    within_range = bool(alphas) and all(
        contract.alpha_min <= alpha <= contract.alpha_max for alpha in alphas
    )
    if not within_range:
        reasons.append(
            _reason(
                "outer_alpha_out_of_range_or_unavailable",
                "outer-band alpha must remain inside the theme range",
                expected_min=contract.alpha_min,
                expected_max=contract.alpha_max,
                observed=alphas,
            )
        )

    resolved = (
        resolution.get("resolved") if resolution.get("available") is True else None
    )
    changed = len(set(alphas)) > 1
    range_tolerance = 6
    range_covered = bool(alphas) and (
        min(alphas) <= contract.alpha_min + range_tolerance
        and max(alphas) >= contract.alpha_max - range_tolerance
    )
    cycle_tolerance_ms = max(50, interval_ms)
    alpha_tolerance = 8
    cycle_pairs = [
        {
            "first_elapsed_ms": first_elapsed,
            "second_elapsed_ms": second_elapsed,
            "delta_ms": second_elapsed - first_elapsed,
            "alpha_delta": abs(second_alpha - first_alpha),
        }
        for index, (first_elapsed, first_alpha) in enumerate(points)
        for second_elapsed, second_alpha in points[index + 1 :]
        if abs((second_elapsed - first_elapsed) - contract.pulse_period_ms)
        <= cycle_tolerance_ms
        and abs(second_alpha - first_alpha) <= alpha_tolerance
    ]
    coverage_ms = points[-1][0] - points[0][0] if len(points) >= 2 else 0
    gaps = [right[0] - left[0] for left, right in pairwise(points)]
    dense_enough = bool(gaps) and max(gaps) <= contract.pulse_period_ms // 8
    cosine_fit: dict[str, Any] = {
        "available": False,
        "phase_ms": None,
        "max_alpha_error": None,
        "rms_alpha_error": None,
    }
    cosine_tolerance = 6

    if resolved == "reduce":
        fixed_max = bool(alphas) and all(
            alpha == contract.alpha_max for alpha in alphas
        )
        if not fixed_max:
            reasons.append(
                _reason(
                    "reduced_motion_not_fixed_at_max",
                    "reduced motion requires a fixed outer alpha at alpha_max",
                    expected=contract.alpha_max,
                    observed=alphas,
                )
            )
        contract_kind = "fixed_max_reduced_motion"
        range_covered = fixed_max
        cycle_matched = fixed_max
    elif resolved == "animate":
        fixed_max = False
        cycle_matched = bool(cycle_pairs)
        if coverage_ms < contract.pulse_period_ms:
            reasons.append(
                _reason(
                    "breathing_cycle_coverage_insufficient",
                    "temporal evidence does not span one complete breathing period",
                    observed_ms=coverage_ms,
                    required_ms=contract.pulse_period_ms,
                )
            )
        if not dense_enough:
            reasons.append(
                _reason(
                    "breathing_sampling_too_sparse",
                    "sample gaps exceed one eighth of the breathing period",
                    max_gap_ms=max(gaps) if gaps else None,
                    allowed_ms=contract.pulse_period_ms // 8,
                )
            )
        if not changed:
            reasons.append(
                _reason(
                    "outer_band_alpha_did_not_change",
                    "animated motion requires observable outer-band alpha change",
                )
            )
        if not range_covered:
            reasons.append(
                _reason(
                    "breathing_alpha_range_not_covered",
                    "samples did not reach both alpha extrema within tolerance",
                    tolerance=range_tolerance,
                    observed_min=min(alphas) if alphas else None,
                    observed_max=max(alphas) if alphas else None,
                )
            )
        if not cycle_matched:
            reasons.append(
                _reason(
                    "breathing_period_not_matched",
                    "no alpha pair matched the configured breathing period",
                    period_ms=contract.pulse_period_ms,
                    time_tolerance_ms=cycle_tolerance_ms,
                    alpha_tolerance=alpha_tolerance,
                )
            )
        cosine_fit = _fit_cosine_alpha(points, contract)
        # At the steepest point, 33 ms of refresh/read timing uncertainty changes
        # 42*cos(2*pi*t/1800) by at most 4.84 alpha units; byte rounding keeps 6
        # units conservative while still rejecting triangular and sawtooth waves.
        cosine_shape_matched = (
            cosine_fit["available"] is True
            and isinstance(cosine_fit["max_alpha_error"], (int, float))
            and cosine_fit["max_alpha_error"] <= cosine_tolerance
        )
        if not cosine_shape_matched:
            reasons.append(
                _reason(
                    "breathing_cosine_shape_mismatch",
                    "outer-band opacity does not match the fixed cosine breathing curve",
                    tolerance=cosine_tolerance,
                    fit=cosine_fit,
                )
            )
        contract_kind = "breathing_cycle"
    else:
        fixed_max = False
        cycle_matched = False
        contract_kind = "unavailable"
        reasons.append(
            _reason(
                "resolved_motion_unavailable",
                "the resolved indicator motion policy is not authoritative",
                resolution=dict(resolution),
            )
        )

    return {
        "accepted": not reasons,
        "contract": contract_kind,
        "resolution": dict(resolution),
        "sample_count": len(points),
        "coverage_ms": coverage_ms,
        "outer_alpha_min": min(alphas) if alphas else None,
        "outer_alpha_max": max(alphas) if alphas else None,
        "outer_alpha_changed": changed,
        "outer_alpha_within_range": within_range,
        "range_covered_with_tolerance": range_covered,
        "cycle_period_matched_with_tolerance": cycle_matched,
        "fixed_at_alpha_max": fixed_max,
        "cosine_fit_phase_ms": cosine_fit["phase_ms"],
        "cosine_fit_max_alpha_error": cosine_fit["max_alpha_error"],
        "cosine_fit_rms_alpha_error": cosine_fit["rms_alpha_error"],
        "cosine_fit_tolerance": cosine_tolerance,
        "cycle_pairs": cycle_pairs,
        "blocking_reasons": reasons,
    }


def resolve_motion(source: ObservationSource, requested: str) -> dict[str, Any]:
    if requested in {"animate", "reduce"}:
        return {
            "available": True,
            "requested": requested,
            "resolved": requested,
            "source": "cli_motion_mode",
            "acceptance_authoritative": False,
            "diagnostic_only": True,
        }
    preference = source.system_animation_enabled()
    if preference.get("available") is not True:
        return _unavailable(
            "system_animation_preference_unavailable",
            "auto motion cannot be resolved without the system animation preference",
            preference=dict(preference),
        )
    return {
        "available": True,
        "requested": "auto",
        "resolved": "animate" if preference.get("value") is True else "reduce",
        "source": preference.get("source", "system_animation_preference"),
        "acceptance_authoritative": True,
    }


def evaluate_migration_samples(
    samples: Sequence[Mapping[str, Any]],
    monitors: Sequence[Mapping[str, Any]],
    *,
    stable_samples_required: int,
) -> dict[str, Any]:
    """Require stable, converged target evidence on two real mixed-DPI monitors."""

    topology = {
        str(monitor.get("hmonitor")): monitor
        for monitor in monitors
        if monitor.get("active", True)
    }
    reasons: list[dict[str, Any]] = []
    maximum_run: dict[str, int] = {}
    stable_observations: dict[str, list[Mapping[str, Any]]] = {}
    current_handle: str | None = None
    current_run: list[Mapping[str, Any]] = []

    def finish_run() -> None:
        nonlocal current_handle, current_run
        if current_handle is not None:
            maximum_run[current_handle] = max(
                maximum_run.get(current_handle, 0), len(current_run)
            )
            if len(current_run) >= stable_samples_required:
                stable_observations.setdefault(current_handle, []).extend(current_run)
        current_handle = None
        current_run = []

    for sample in samples:
        if sample.get("converged") is not True:
            finish_run()
            continue
        handle = sample.get("monitor_handle")
        dpi = sample.get("monitor_dpi")
        bounds = sample.get("dwm_bounds")
        monitor = topology.get(str(handle))
        if (
            monitor is None
            or not isinstance(dpi, int)
            or not isinstance(bounds, Mapping)
        ):
            finish_run()
            reasons.append(
                _reason(
                    "converged_sample_monitor_binding_unavailable",
                    "a converged sample lacks an active HMONITOR, DPI, or DWM bounds binding",
                    monitor_handle=handle,
                )
            )
            continue
        monitor_dpi = monitor.get("effective_dpi", {})
        expected_dpi = (
            int(monitor_dpi["x"])
            if monitor_dpi.get("available") is True
            and monitor_dpi.get("authority") == "private_hidden_pmv2_probe"
            else None
        )
        if dpi != expected_dpi:
            finish_run()
            reasons.append(
                _reason(
                    "converged_sample_monitor_dpi_mismatch",
                    "sample target DPI does not match the authoritative topology DPI",
                    monitor_handle=handle,
                    expected=expected_dpi,
                    observed=dpi,
                )
            )
            continue
        monitor_bounds = monitor.get("bounds")
        intersects = isinstance(monitor_bounds, Mapping) and (
            max(int(bounds["left"]), int(monitor_bounds["left"]))
            < min(int(bounds["right"]), int(monitor_bounds["right"]))
            and max(int(bounds["top"]), int(monitor_bounds["top"]))
            < min(int(bounds["bottom"]), int(monitor_bounds["bottom"]))
        )
        if not intersects:
            finish_run()
            reasons.append(
                _reason(
                    "converged_sample_outside_bound_monitor",
                    "sample DWM bounds do not intersect the bound HMONITOR",
                    monitor_handle=handle,
                    dwm_bounds=dict(bounds),
                )
            )
            continue
        if str(handle) != current_handle:
            finish_run()
            current_handle = str(handle)
        current_run.append(sample)
    finish_run()

    stable_handles = sorted(stable_observations)
    stable_dpis = sorted(
        {
            int(observation["monitor_dpi"])
            for observations in stable_observations.values()
            for observation in observations
        }
    )
    negative_visit = any(
        (
            int(observation["dwm_bounds"]["left"]) < 0
            or int(observation["dwm_bounds"]["top"]) < 0
        )
        and (
            int(topology[handle]["bounds"]["left"]) < 0
            or int(topology[handle]["bounds"]["top"]) < 0
        )
        for handle, observations in stable_observations.items()
        for observation in observations
    )
    if len(stable_handles) < 2:
        reasons.append(
            _reason(
                "stable_mixed_monitor_visits_required",
                "two HMONITORs must each have consecutive converged samples",
                stable_samples_required=stable_samples_required,
                maximum_consecutive_samples=maximum_run,
            )
        )
    if len(stable_dpis) < 2:
        reasons.append(
            _reason(
                "stable_mixed_dpi_visits_required",
                "stable target visits do not cover two effective DPI values",
                observed=stable_dpis,
            )
        )
    if not negative_visit:
        reasons.append(
            _reason(
                "stable_negative_coordinate_visit_required",
                "no stable converged target visit has negative DWM coordinates",
            )
        )
    return {
        "accepted": not reasons,
        "stable_samples_required": stable_samples_required,
        "maximum_consecutive_samples": maximum_run,
        "stable_monitor_handles": stable_handles,
        "stable_effective_dpis": stable_dpis,
        "negative_coordinate_visit": negative_visit,
        "blocking_reasons": reasons,
    }


@dataclass(frozen=True)
class ProbeConfig:
    target_pid: int
    target_hwnd: int
    interval_ms: int = 100
    duration_ms: int = 3_000
    require_cleanup: bool = False
    motion_mode: str = "auto"

    def __post_init__(self) -> None:
        if self.target_pid <= 0:
            raise ValueError("target_pid must be positive")
        if self.target_hwnd <= 0:
            raise ValueError("target_hwnd must be positive")
        if self.interval_ms <= 0:
            raise ValueError("interval_ms must be positive")
        if self.duration_ms < 0:
            raise ValueError("duration_ms must not be negative")
        if self.motion_mode not in {"auto", "animate", "reduce"}:
            raise ValueError("motion_mode must be auto, animate, or reduce")


@dataclass(frozen=True)
class ProbeRunResult:
    exit_code: int
    converged: bool
    cleanup_status: str


class ObservationSource(Protocol):
    """Injectable read-only boundary around platform observation."""

    def enumerate_monitors(self) -> Sequence[Mapping[str, Any]]: ...

    def observe(self, target_pid: int, target_hwnd: int) -> Mapping[str, Any]: ...

    def system_animation_enabled(self) -> Mapping[str, Any]: ...


class ProbeFailure(RuntimeError):
    def __init__(self, kind: str, message: str, **details: Any) -> None:
        super().__init__(message)
        self.reason = _reason(kind, message, **details)


def _available(value: Any, source: str) -> dict[str, Any]:
    return {"available": True, "value": value, "source": source}


def _unavailable(kind: str, message: str, **details: Any) -> dict[str, Any]:
    return {"available": False, "reason": _reason(kind, message, **details)}


def evaluate_private_probe_binding(
    intended_hmonitor: str, observed_hmonitor: str
) -> dict[str, Any]:
    if intended_hmonitor != observed_hmonitor:
        return _unavailable(
            "private_pmv2_probe_monitor_mismatch",
            "the private hidden probe did not land on the intended HMONITOR",
            intended_hmonitor=intended_hmonitor,
            observed_hmonitor=observed_hmonitor,
        )
    return _available(observed_hmonitor, "MonitorFromWindow(private_hidden_pmv2_probe)")


def _reason(kind: str, message: str, **details: Any) -> dict[str, Any]:
    return {"kind": kind, "message": message, **details}


def evaluate_topology(
    monitors: Sequence[Mapping[str, Any]], contract: FrameContract
) -> dict[str, Any]:
    """Evaluate the physical topology required by the mixed-DPI acceptance run."""

    active = [monitor for monitor in monitors if monitor.get("active", True)]
    reasons: list[dict[str, Any]] = []
    if len(active) < 2:
        reasons.append(
            _reason(
                "insufficient_active_monitors",
                "mixed-monitor acceptance requires at least two active monitors",
                observed=len(active),
                required=2,
            )
        )

    has_negative_coordinate = any(
        int(monitor["bounds"][axis]) < 0
        for monitor in active
        for axis in ("left", "top")
    )
    if not has_negative_coordinate:
        reasons.append(
            _reason(
                "negative_coordinate_monitor_required",
                "no active monitor occupies negative virtual-desktop coordinates",
            )
        )

    unavailable_dpi = [
        monitor
        for monitor in active
        if not bool(monitor.get("effective_dpi", {}).get("available"))
    ]
    untrusted_dpi = [
        monitor
        for monitor in active
        if bool(monitor.get("effective_dpi", {}).get("available"))
        and monitor.get("effective_dpi", {}).get("authority")
        != "private_hidden_pmv2_probe"
    ]
    if unavailable_dpi:
        reasons.append(
            _reason(
                "monitor_effective_dpi_unavailable",
                "effective DPI could not be read for every active monitor",
                hmonitors=[monitor.get("hmonitor") for monitor in unavailable_dpi],
            )
        )
        effective_dpis: set[int] = set()
    elif untrusted_dpi:
        reasons.append(
            _reason(
                "monitor_effective_dpi_authority_untrusted",
                "effective DPI was not measured by a private hidden PMv2 probe window",
                hmonitors=[monitor.get("hmonitor") for monitor in untrusted_dpi],
            )
        )
        effective_dpis = set()
    else:
        effective_dpis = {
            int(monitor["effective_dpi"]["x"])
            for monitor in active
            if int(monitor["effective_dpi"]["x"]) == int(monitor["effective_dpi"]["y"])
        }
        asymmetric = [
            monitor.get("hmonitor")
            for monitor in active
            if int(monitor["effective_dpi"]["x"]) != int(monitor["effective_dpi"]["y"])
        ]
        if asymmetric:
            reasons.append(
                _reason(
                    "asymmetric_effective_dpi_unsupported",
                    "monitor effective DPI differs between axes",
                    hmonitors=asymmetric,
                )
            )
        elif len(effective_dpis) < 2:
            reasons.append(
                _reason(
                    "mixed_effective_dpi_required",
                    "active monitors do not expose at least two effective DPI values",
                    observed=sorted(effective_dpis),
                )
            )

    expected = [
        {
            "dpi": dpi,
            "thickness_dip": contract.thickness_dip,
            "thickness_px": contract.thickness_px(dpi),
            "gradient_steps": contract.gradient_steps,
        }
        for dpi in sorted(effective_dpis)
    ]
    return {
        "eligible": not reasons,
        "active_monitor_count": len(active),
        "has_negative_coordinate_monitor": has_negative_coordinate,
        "distinct_effective_dpi": sorted(effective_dpis),
        "expected_frame_contracts": expected,
        "blocking_reasons": reasons,
    }


def _available_value(field: Any) -> Any | None:
    if isinstance(field, Mapping) and field.get("available") is True:
        return field.get("value")
    return None


def _rect_size(bounds: Mapping[str, Any]) -> tuple[int, int]:
    return (
        int(bounds["right"]) - int(bounds["left"]),
        int(bounds["bottom"]) - int(bounds["top"]),
    )


def _scale_dip(value: int, dpi: int) -> int:
    return (value * dpi + 48) // 96


def expected_banner_geometry(
    target_bounds: Mapping[str, Any],
    monitor_bounds: Mapping[str, Any],
    work_area: Mapping[str, Any],
    dpi: int,
) -> dict[str, Any]:
    """Mirror the fixed target-scoped 480x44 DIP banner placement contract."""

    target_width, _target_height = _rect_size(target_bounds)
    height = _scale_dip(44, dpi)
    available_width = max(
        1, int(work_area["right"]) - int(work_area["left"]) - _scale_dip(16, dpi)
    )
    width = min(_scale_dip(480, dpi), available_width)
    gap = _scale_dip(8, dpi)
    inset = _scale_dip(16, dpi)
    fullscreen = (
        abs(int(target_bounds["left"]) - int(monitor_bounds["left"])) <= 2
        and abs(int(target_bounds["top"]) - int(monitor_bounds["top"])) <= 2
        and abs(int(target_bounds["right"]) - int(monitor_bounds["right"])) <= 2
        and abs(int(target_bounds["bottom"]) - int(monitor_bounds["bottom"])) <= 2
    )
    inside_target = fullscreen or int(target_bounds["top"]) - height - gap < int(
        work_area["top"]
    )
    if inside_target:
        top = min(
            int(target_bounds["top"]) + inset,
            int(work_area["bottom"]) - height,
        )
    else:
        top = int(target_bounds["top"]) - height - gap
    unclamped_left = int(target_bounds["left"]) + (target_width - width) // 2
    left = min(
        max(unclamped_left, int(work_area["left"])),
        int(work_area["right"]) - width,
    )
    return {
        "left": left,
        "top": top,
        "right": left + width,
        "bottom": top + height,
        "width": width,
        "height": height,
        "inside_target": inside_target,
    }


def _overlay_contract_reasons(
    window: Mapping[str, Any],
    expected_dpi: int,
    expected_monitor_handle: str,
    *,
    require_visible: bool,
    require_target_z_order: bool,
) -> list[dict[str, Any]]:
    hwnd = window.get("hwnd")
    reasons: list[dict[str, Any]] = []
    if _available_value(window.get("dpi")) != expected_dpi:
        reasons.append(
            _reason(
                "indicator_dpi_mismatch_or_unavailable",
                "indicator DPI does not match the target monitor effective DPI",
                hwnd=hwnd,
                expected=expected_dpi,
                observed=_available_value(window.get("dpi")),
            )
        )
    layered = window.get("layered")
    if not isinstance(layered, Mapping) or layered.get("available") is not True:
        reasons.append(
            _reason(
                "layered_state_unavailable",
                "layered window state could not be read",
                hwnd=hwnd,
            )
        )
    elif layered.get("value") is not True:
        reasons.append(
            _reason(
                "indicator_not_layered", "indicator window is not layered", hwnd=hwnd
            )
        )
    else:
        layered_flags = _available_value(layered.get("flags"))
        if not isinstance(layered_flags, int) or not layered_flags & 0x2:
            reasons.append(
                _reason(
                    "layered_alpha_flag_missing_or_unavailable",
                    "layered opacity is not authoritative without LWA_ALPHA",
                    hwnd=hwnd,
                    observed=layered_flags,
                )
            )
    if _available_value(window.get("display_affinity")) != 17:
        reasons.append(
            _reason(
                "capture_exclusion_missing_or_unavailable",
                "indicator does not expose WDA_EXCLUDEFROMCAPTURE",
                hwnd=hwnd,
                observed=_available_value(window.get("display_affinity")),
            )
        )
    observed_monitor = _available_value(window.get("monitor_handle"))
    if observed_monitor != expected_monitor_handle:
        reasons.append(
            _reason(
                "indicator_monitor_mismatch_or_unavailable",
                "indicator HWND is not bound to the exact target HMONITOR",
                hwnd=hwnd,
                expected=expected_monitor_handle,
                observed=observed_monitor,
            )
        )
    styles = _available_value(window.get("extended_styles"))
    if not isinstance(styles, Mapping) or styles.get("topmost") is not False:
        reasons.append(
            _reason(
                "indicator_global_topmost_forbidden",
                "target-scoped indicators must not join the global topmost band",
                hwnd=hwnd,
            )
        )
    if not isinstance(styles, Mapping) or styles.get("no_activate") is not True:
        reasons.append(
            _reason(
                "indicator_noactivate_style_required",
                "indicator HWNDs must expose WS_EX_NOACTIVATE",
                hwnd=hwnd,
            )
        )
    if not isinstance(styles, Mapping) or styles.get("tool_window") is not True:
        reasons.append(
            _reason(
                "indicator_toolwindow_style_required",
                "indicator HWNDs must expose WS_EX_TOOLWINDOW",
                hwnd=hwnd,
            )
        )
    if not isinstance(styles, Mapping) or styles.get("transparent") is not True:
        reasons.append(
            _reason(
                "indicator_transparent_style_required",
                "indicator HWNDs must expose WS_EX_TRANSPARENT for input pass-through",
                hwnd=hwnd,
            )
        )
    owner = window.get("owner_hwnd")
    if not isinstance(owner, Mapping) or owner.get("available") is not True:
        reasons.append(
            _reason(
                "indicator_owner_unavailable",
                "indicator owner HWND could not be read",
                hwnd=hwnd,
            )
        )
    elif owner.get("value") is not None:
        reasons.append(
            _reason(
                "indicator_owner_must_be_null",
                "target-scoped indicators are intentionally ownerless",
                hwnd=hwnd,
                observed=owner.get("value"),
            )
        )
    if require_visible and _available_value(window.get("visible")) is not True:
        reasons.append(
            _reason(
                "indicator_not_visible_or_visibility_unavailable",
                "indicator window is not observably visible",
                hwnd=hwnd,
            )
        )
    if require_target_z_order:
        z_order = window.get("z_order")
        if (
            not isinstance(z_order, Mapping)
            or z_order.get("available") is not True
            or z_order.get("relation_to_target") != "above"
        ):
            reasons.append(
                _reason(
                    "target_scoped_z_order_unproven",
                    "indicator is not observably above the exact target",
                    hwnd=hwnd,
                )
            )
    return reasons


def evaluate_sample(
    target: Mapping[str, Any],
    indicator_windows: Sequence[Mapping[str, Any]],
    contract: FrameContract,
) -> dict[str, Any]:
    """Evaluate one immutable Win32 observation without assigning hidden roles."""

    reasons: list[dict[str, Any]] = []
    requested_pid = int(target.get("requested_pid", 0))
    if _available_value(target.get("is_window")) is not True:
        reasons.append(
            _reason(
                "target_hwnd_invalid", "the requested target HWND is not a live window"
            )
        )
    actual_pid = _available_value(target.get("process_id"))
    if actual_pid != requested_pid:
        reasons.append(
            _reason(
                "target_pid_mismatch_or_unavailable",
                "the target HWND does not belong to the requested PID",
                requested=requested_pid,
                observed=actual_pid,
            )
        )
    target_bounds = _available_value(target.get("dwm_extended_frame_bounds"))
    if not isinstance(target_bounds, Mapping):
        reasons.append(
            _reason(
                "target_dwm_bounds_unavailable",
                "DWM extended frame bounds are required for exact geometry acceptance",
            )
        )
    monitor_dpi_field = target.get("monitor_effective_dpi")
    monitor_dpi = _available_value(monitor_dpi_field)
    if not isinstance(monitor_dpi, int) or monitor_dpi <= 0:
        reasons.append(
            _reason(
                "target_monitor_effective_dpi_unavailable",
                "the target monitor effective DPI is required",
            )
        )
    elif (
        not isinstance(monitor_dpi_field, Mapping)
        or monitor_dpi_field.get("authority") != "private_hidden_pmv2_probe"
    ):
        reasons.append(
            _reason(
                "target_monitor_effective_dpi_authority_untrusted",
                "target monitor DPI was not measured by the private hidden PMv2 probe",
            )
        )
    target_monitor_handle = _available_value(target.get("monitor_handle"))
    if not isinstance(target_monitor_handle, str):
        reasons.append(
            _reason(
                "target_monitor_handle_unavailable",
                "the exact target HMONITOR is required for indicator binding",
            )
        )
    target_monitor_bounds = _available_value(target.get("monitor_bounds"))
    target_monitor_work_area = _available_value(target.get("monitor_work_area"))
    if not isinstance(target_monitor_bounds, Mapping) or not isinstance(
        target_monitor_work_area, Mapping
    ):
        reasons.append(
            _reason(
                "target_monitor_geometry_unavailable",
                "banner acceptance requires target monitor bounds and work area",
            )
        )

    banners = [
        window
        for window in indicator_windows
        if window.get("class_name") == "DccCuaControlBanner"
    ]
    frame_family = [
        window
        for window in indicator_windows
        if window.get("class_name") == "DccCuaControlFrame"
    ]

    frame_like: list[Mapping[str, Any]] = []
    probe_like: list[Mapping[str, Any]] = []
    if isinstance(target_bounds, Mapping):
        target_width, target_height = _rect_size(target_bounds)
        for window in frame_family:
            region = window.get("region")
            window_bounds = _available_value(window.get("window_rect"))
            frame_bounds = _available_value(window.get("dwm_extended_frame_bounds"))
            if (
                isinstance(region, Mapping)
                and region.get("available") is True
                and region.get("complexity") in {"simple", "complex"}
                and isinstance(region.get("bounds"), Mapping)
                and frame_bounds == target_bounds
            ):
                frame_like.append(window)
                continue
            if (
                isinstance(region, Mapping)
                and region.get("available") is True
                and region.get("complexity") == "null"
                and isinstance(window_bounds, Mapping)
                and _rect_size(window_bounds) == (1, 1)
                and _available_value(window.get("visible")) is False
            ):
                probe_like.append(window)
    else:
        target_width = target_height = 0

    if len(banners) != 1:
        reasons.append(
            _reason(
                "exactly_one_banner_required",
                "acceptance requires exactly one DccCuaControlBanner candidate",
                observed=len(banners),
            )
        )
    if len(frame_like) != contract.gradient_steps:
        reasons.append(
            _reason(
                "gradient_frame_count_mismatch",
                "the observed target-sized frame region count does not match the contract",
                observed=len(frame_like),
                expected=contract.gradient_steps,
            )
        )
    if len(probe_like) != 1:
        reasons.append(
            _reason(
                "dpi_probe_structural_candidate_count_mismatch",
                "one hidden 1x1 frame-class candidate is required as DPI-probe evidence",
                observed=len(probe_like),
                expected=1,
            )
        )
    classified_frame_hwnds = {
        str(window.get("hwnd")) for window in [*frame_like, *probe_like]
    }
    unclassified_frames = [
        window
        for window in frame_family
        if str(window.get("hwnd")) not in classified_frame_hwnds
    ]
    if unclassified_frames:
        reasons.append(
            _reason(
                "unclassified_indicator_frame_windows",
                "every DccCuaControlFrame HWND must classify as one band or the DPI probe",
                hwnds=[window.get("hwnd") for window in unclassified_frames],
            )
        )

    indicator_family = [*banners, *frame_like, *probe_like]
    visible_indicator_family = [*banners, *frame_like]
    family_process_ids = [
        _available_value(window.get("process_id")) for window in indicator_family
    ]
    if any(not isinstance(process_id, int) for process_id in family_process_ids):
        reasons.append(
            _reason(
                "indicator_family_process_unavailable",
                "every indicator HWND must expose its owning process",
                observed=family_process_ids,
            )
        )
    elif len(set(family_process_ids)) != 1:
        reasons.append(
            _reason(
                "indicator_family_process_mismatch",
                "banner, frame bands, and DPI probe do not share one process",
                observed=sorted(set(family_process_ids)),
            )
        )
    family_thread_ids = [
        _available_value(window.get("thread_id")) for window in indicator_family
    ]
    if any(not isinstance(thread_id, int) for thread_id in family_thread_ids):
        reasons.append(
            _reason(
                "indicator_family_thread_unavailable",
                "every indicator HWND must expose its owning GUI thread",
                observed=family_thread_ids,
            )
        )
    elif len(set(family_thread_ids)) != 1:
        reasons.append(
            _reason(
                "indicator_family_thread_mismatch",
                "banner, frame bands, and DPI probe do not share one GUI thread",
                observed=sorted(set(family_thread_ids)),
            )
        )

    z_order_records = [window.get("z_order") for window in visible_indicator_family]
    z_order_indices = [
        record.get("desktop_index_top_to_bottom")
        if isinstance(record, Mapping) and record.get("available") is True
        else None
        for record in z_order_records
    ]
    target_z_order_indices = [
        record.get("target_index_top_to_bottom")
        if isinstance(record, Mapping) and record.get("available") is True
        else None
        for record in z_order_records
    ]
    visible_hwnds = [str(window.get("hwnd")) for window in visible_indicator_family]
    target_hwnd = str(target.get("requested_hwnd"))
    successor_by_hwnd = {
        hwnd: record.get("next_hwnd")
        for hwnd, record in zip(visible_hwnds, z_order_records, strict=True)
        if isinstance(record, Mapping) and record.get("available") is True
    }
    visible_hwnd_set = set(visible_hwnds)
    incoming_visible = {
        successor
        for successor in successor_by_hwnd.values()
        if successor in visible_hwnd_set
    }
    top_candidates = visible_hwnd_set - incoming_visible
    target_scoped_z_order_block = False
    walked_hwnds: list[str] = []
    if (
        visible_indicator_family
        and len(visible_hwnd_set) == len(visible_hwnds)
        and len(successor_by_hwnd) == len(visible_hwnds)
        and all(
            successor in visible_hwnd_set or successor == target_hwnd
            for successor in successor_by_hwnd.values()
        )
        and len(top_candidates) == 1
        and list(successor_by_hwnd.values()).count(target_hwnd) == 1
    ):
        current = next(iter(top_candidates))
        seen: set[str] = set()
        while current in successor_by_hwnd and current not in seen:
            seen.add(current)
            walked_hwnds.append(current)
            successor = successor_by_hwnd[current]
            if successor == target_hwnd:
                target_scoped_z_order_block = seen == visible_hwnd_set
                break
            current = str(successor)
    if not target_scoped_z_order_block:
        reasons.append(
            _reason(
                "target_scoped_z_order_block_unproven",
                "GetWindow(GW_HWNDNEXT) must prove one finite visible-HWND chain ending at the exact target",
                indicator_indices=z_order_indices,
                target_indices=target_z_order_indices,
                successor_by_hwnd=successor_by_hwnd,
                walked_hwnds=walked_hwnds,
            )
        )

    expected_banner: dict[str, Any] | None = None
    observed_banner_bounds: Any = None
    banner_region_accepted = False
    if (
        len(banners) == 1
        and isinstance(target_bounds, Mapping)
        and isinstance(target_monitor_bounds, Mapping)
        and isinstance(target_monitor_work_area, Mapping)
        and isinstance(monitor_dpi, int)
    ):
        expected_banner = expected_banner_geometry(
            target_bounds,
            target_monitor_bounds,
            target_monitor_work_area,
            monitor_dpi,
        )
        expected_banner_bounds = {
            edge: expected_banner[edge] for edge in ("left", "top", "right", "bottom")
        }
        banner = banners[0]
        observed_banner_bounds = _available_value(banner.get("window_rect"))
        observed_banner_dwm_bounds = _available_value(
            banner.get("dwm_extended_frame_bounds")
        )
        if (
            observed_banner_bounds != expected_banner_bounds
            or observed_banner_dwm_bounds != expected_banner_bounds
        ):
            reasons.append(
                _reason(
                    "banner_geometry_mismatch_or_unavailable",
                    "banner HWND does not match the fixed target-scoped DIP placement",
                    expected=expected_banner_bounds,
                    observed_window_rect=observed_banner_bounds,
                    observed_dwm_bounds=observed_banner_dwm_bounds,
                )
            )
        region = banner.get("region")
        expected_region_bounds = {
            "left": 0,
            "top": 0,
            "right": expected_banner["width"],
            "bottom": expected_banner["height"],
        }
        point_membership = (
            region.get("point_membership") if isinstance(region, Mapping) else None
        )
        corners_outside = isinstance(point_membership, Mapping) and all(
            point_membership.get(point) is False
            for point in (
                "top_left_corner",
                "top_right_corner",
                "bottom_left_corner",
                "bottom_right_corner",
            )
        )
        body_inside = isinstance(point_membership, Mapping) and all(
            point_membership.get(point) is True
            for point in (
                "top_midpoint",
                "bottom_midpoint",
                "left_midpoint",
                "right_midpoint",
                "center",
            )
        )
        banner_region_accepted = bool(
            isinstance(region, Mapping)
            and region.get("available") is True
            and region.get("complexity") == "complex"
            and region.get("bounds") == expected_region_bounds
            and corners_outside
            and body_inside
        )
        if not banner_region_accepted:
            reasons.append(
                _reason(
                    "banner_rounded_region_unproven",
                    "banner HRGN does not prove a target-sized rounded rectangle",
                    expected_bounds=expected_region_bounds,
                    observed=region,
                )
            )

    expected_bands = contract.bands(monitor_dpi) if isinstance(monitor_dpi, int) else ()
    expected_insets = [
        (band.outer_inset_px, band.inner_inset_px) for band in expected_bands
    ]
    observed_bands: list[tuple[int, int | None, int | None, str | None]] = []
    symmetric_regions = True
    four_sided_rings = True
    observed_edge_bands: dict[str, dict[str, tuple[int, int] | None]] = {}
    for window in frame_like:
        region = window["region"]
        region_bounds = region["bounds"]
        left = int(region_bounds["left"])
        top = int(region_bounds["top"])
        right = int(region_bounds["right"])
        bottom = int(region_bounds["bottom"])
        symmetric_regions &= (
            left == top
            and right == target_width - left
            and bottom == target_height - left
        )
        edge_bands = region.get("edge_bands")
        per_edge: dict[str, tuple[int, int] | None] = {}
        for edge in ("top", "bottom", "left", "right"):
            evidence = edge_bands.get(edge) if isinstance(edge_bands, Mapping) else None
            if (
                isinstance(evidence, Mapping)
                and evidence.get("available") is True
                and isinstance(evidence.get("outer_inset_px"), int)
                and isinstance(evidence.get("inner_inset_px"), int)
            ):
                per_edge[edge] = (
                    int(evidence["outer_inset_px"]),
                    int(evidence["inner_inset_px"]),
                )
            else:
                per_edge[edge] = None
        observed_edge_bands[str(window.get("hwnd"))] = per_edge
        complete_edges = [pair for pair in per_edge.values() if pair is not None]
        ring_is_four_sided = len(complete_edges) == 4 and len(set(complete_edges)) == 1
        four_sided_rings &= ring_is_four_sided
        if ring_is_four_sided:
            observed_outer, observed_inner = complete_edges[0]
            symmetric_regions &= observed_outer == left
        else:
            observed_outer = left
            observed_inner = None
        layered = window.get("layered")
        alpha = None
        if isinstance(layered, Mapping):
            alpha = _available_value(layered.get("alpha"))
        observed_bands.append(
            (
                observed_outer,
                observed_inner,
                alpha if isinstance(alpha, int) else None,
                window.get("hwnd"),
            )
        )
    observed_bands.sort(key=lambda item: item[0])
    observed_insets = [(item[0], item[1]) for item in observed_bands]
    bands_cover = (
        symmetric_regions and four_sided_rings and observed_insets == expected_insets
    )
    observed_alphas = [item[2] for item in observed_bands]
    alpha_monotonic = (
        bool(observed_alphas)
        and all(isinstance(alpha, int) for alpha in observed_alphas)
        and all(int(left) >= int(right) for left, right in pairwise(observed_alphas))
    )
    outer_alpha = observed_alphas[0] if observed_alphas else None
    expected_alphas = (
        [
            outer_alpha
            * (contract.gradient_steps - index)
            * (contract.gradient_steps - index)
            // (contract.gradient_steps * contract.gradient_steps)
            for index in range(contract.gradient_steps)
        ]
        if isinstance(outer_alpha, int)
        else []
    )
    alpha_profile_matches = observed_alphas == expected_alphas
    if not bands_cover:
        reasons.append(
            _reason(
                "gradient_regions_do_not_cover_contract",
                "frame regions do not prove every expected outer-to-inner ring band",
                expected_band_insets_px=expected_insets,
                observed_band_insets_px=observed_insets,
            )
        )
    if not alpha_monotonic:
        reasons.append(
            _reason(
                "gradient_alpha_not_monotonic_or_unavailable",
                "frame alpha must fade monotonically from edge toward center",
            )
        )
    if not alpha_profile_matches:
        reasons.append(
            _reason(
                "gradient_alpha_profile_mismatch",
                "frame alpha does not match the quadratic inward fade from the current edge opacity",
                expected=expected_alphas,
                observed=observed_alphas,
            )
        )

    if isinstance(monitor_dpi, int):
        for window in [*banners, *frame_like]:
            reasons.extend(
                _overlay_contract_reasons(
                    window,
                    monitor_dpi,
                    str(target_monitor_handle),
                    require_visible=True,
                    require_target_z_order=False,
                )
            )
        for window in probe_like:
            reasons.extend(
                _overlay_contract_reasons(
                    window,
                    monitor_dpi,
                    str(target_monitor_handle),
                    require_visible=False,
                    require_target_z_order=False,
                )
            )

    logical_role = {
        "available": False,
        "reason": _reason(
            "logical_role_not_exposed_by_win32",
            "DccCuaControlFrame is shared by frame bands and the hidden DPI probe",
        ),
    }
    return {
        "converged": not reasons,
        "blocking_reasons": reasons,
        "banner_contract": {
            "observed_banner_count": len(banners),
            "candidate_hwnds": [window.get("hwnd") for window in banners],
            "expected_geometry": expected_banner,
            "observed_window_rect": observed_banner_bounds,
            "rounded_region_proven": banner_region_accepted,
        },
        "indicator_family": {
            "process_ids": family_process_ids,
            "thread_ids": family_thread_ids,
            "unclassified_frame_hwnds": [
                window.get("hwnd") for window in unclassified_frames
            ],
            "target_scoped_z_order_block": target_scoped_z_order_block,
            "visible_z_order_indices": z_order_indices,
            "target_z_order_indices": target_z_order_indices,
            "successor_by_hwnd": successor_by_hwnd,
            "walked_hwnds": walked_hwnds,
        },
        "frame_contract": {
            "expected_frame_count": contract.gradient_steps,
            "observed_frame_count": len(frame_like),
            "geometry_source": "DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)",
            "expected_thickness_px": (
                contract.thickness_px(monitor_dpi)
                if isinstance(monitor_dpi, int)
                else None
            ),
            "expected_band_insets_px": expected_insets,
            "observed_band_insets_px": observed_insets,
            "outer_band_alpha": outer_alpha,
            "bands_cover_without_gaps": bands_cover,
            "four_sided_rings": four_sided_rings,
            "observed_edge_bands_by_hwnd": observed_edge_bands,
            "alpha_monotonic_inward": alpha_monotonic,
            "expected_alpha_profile": expected_alphas,
            "alpha_profile_matches": alpha_profile_matches,
            "candidate_hwnds_by_inset": [item[3] for item in observed_bands],
        },
        "dpi_probe_evidence": {
            "candidate_hwnds": [window.get("hwnd") for window in probe_like],
            "logical_role": logical_role,
        },
    }


class Win32ObservationSource:
    """Observer that mutates only its own never-shown PMv2 DPI probe HWND."""

    _BANNER_CLASS = "DccCuaControlBanner"
    _FRAME_CLASS = "DccCuaControlFrame"
    _DWMWA_EXTENDED_FRAME_BOUNDS = 9
    _MONITOR_DEFAULTTONEAREST = 2
    _MONITORINFOF_PRIMARY = 1
    _GWL_EXSTYLE = -20
    _WS_EX_TOPMOST = 0x00000008
    _WS_EX_TRANSPARENT = 0x00000020
    _WS_EX_LAYERED = 0x00080000
    _WS_EX_TOOLWINDOW = 0x00000080
    _WS_EX_NOACTIVATE = 0x08000000
    _WS_POPUP = 0x80000000
    _SWP_NOSIZE = 0x0001
    _SWP_NOZORDER = 0x0004
    _SWP_NOACTIVATE = 0x0010
    _SPI_GETCLIENTAREAANIMATION = 0x1042
    _DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 = -4
    _GW_HWNDNEXT = 2
    _GW_HWNDPREV = 3
    _GW_OWNER = 4
    _REGION_COMPLEXITY: ClassVar[dict[int, str]] = {
        1: "null",
        2: "simple",
        3: "complex",
    }

    def __init__(self) -> None:
        if sys.platform != "win32":
            raise ProbeFailure(
                "platform_unsupported",
                "the native acceptance observer requires Windows",
                platform=sys.platform,
            )
        from ctypes import wintypes

        self._wintypes = wintypes

        class Rect(ctypes.Structure):
            _fields_ = [
                ("left", wintypes.LONG),
                ("top", wintypes.LONG),
                ("right", wintypes.LONG),
                ("bottom", wintypes.LONG),
            ]

        class MonitorInfoEx(ctypes.Structure):
            _fields_ = [
                ("cbSize", wintypes.DWORD),
                ("rcMonitor", Rect),
                ("rcWork", Rect),
                ("dwFlags", wintypes.DWORD),
                ("szDevice", wintypes.WCHAR * 32),
            ]

        self._Rect = Rect
        self._MonitorInfoEx = MonitorInfoEx
        self._monitor_callback_type = ctypes.WINFUNCTYPE(
            wintypes.BOOL,
            wintypes.HANDLE,
            wintypes.HDC,
            ctypes.POINTER(Rect),
            wintypes.LPARAM,
        )
        self._window_callback_type = ctypes.WINFUNCTYPE(
            wintypes.BOOL, wintypes.HWND, wintypes.LPARAM
        )
        self._user32 = ctypes.WinDLL("user32", use_last_error=True)
        self._gdi32 = ctypes.WinDLL("gdi32", use_last_error=True)
        self._dwmapi = ctypes.WinDLL("dwmapi", use_last_error=True)
        self._monitor_dpi_cache: dict[int, dict[str, Any]] = {}
        self._monitor_geometry_cache: dict[int, dict[str, Any]] = {}
        self._bind_functions()

    def _bind_functions(self) -> None:
        w = self._wintypes
        rect_pointer = ctypes.POINTER(self._Rect)
        monitor_info_pointer = ctypes.POINTER(self._MonitorInfoEx)

        self._user32.EnumDisplayMonitors.argtypes = [
            w.HDC,
            rect_pointer,
            self._monitor_callback_type,
            w.LPARAM,
        ]
        self._user32.EnumDisplayMonitors.restype = w.BOOL
        self._user32.GetMonitorInfoW.argtypes = [w.HANDLE, monitor_info_pointer]
        self._user32.GetMonitorInfoW.restype = w.BOOL
        self._user32.EnumWindows.argtypes = [self._window_callback_type, w.LPARAM]
        self._user32.EnumWindows.restype = w.BOOL
        self._user32.GetClassNameW.argtypes = [w.HWND, w.LPWSTR, ctypes.c_int]
        self._user32.GetClassNameW.restype = ctypes.c_int
        self._user32.IsWindow.argtypes = [w.HWND]
        self._user32.IsWindow.restype = w.BOOL
        self._user32.IsWindowVisible.argtypes = [w.HWND]
        self._user32.IsWindowVisible.restype = w.BOOL
        self._user32.GetWindowRect.argtypes = [w.HWND, rect_pointer]
        self._user32.GetWindowRect.restype = w.BOOL
        self._user32.GetWindowThreadProcessId.argtypes = [
            w.HWND,
            ctypes.POINTER(w.DWORD),
        ]
        self._user32.GetWindowThreadProcessId.restype = w.DWORD
        self._user32.MonitorFromWindow.argtypes = [w.HWND, w.DWORD]
        self._user32.MonitorFromWindow.restype = w.HANDLE
        self._user32.GetWindow.argtypes = [w.HWND, w.UINT]
        self._user32.GetWindow.restype = w.HWND
        self._user32.GetWindowDisplayAffinity.argtypes = [
            w.HWND,
            ctypes.POINTER(w.DWORD),
        ]
        self._user32.GetWindowDisplayAffinity.restype = w.BOOL
        self._user32.CreateWindowExW.argtypes = [
            w.DWORD,
            w.LPCWSTR,
            w.LPCWSTR,
            w.DWORD,
            ctypes.c_int,
            ctypes.c_int,
            ctypes.c_int,
            ctypes.c_int,
            w.HWND,
            w.HANDLE,
            w.HANDLE,
            w.LPVOID,
        ]
        self._user32.CreateWindowExW.restype = w.HWND
        self._user32.SetWindowPos.argtypes = [
            w.HWND,
            w.HWND,
            ctypes.c_int,
            ctypes.c_int,
            ctypes.c_int,
            ctypes.c_int,
            w.UINT,
        ]
        self._user32.SetWindowPos.restype = w.BOOL
        self._user32.DestroyWindow.argtypes = [w.HWND]
        self._user32.DestroyWindow.restype = w.BOOL
        self._user32.SystemParametersInfoW.argtypes = [
            w.UINT,
            w.UINT,
            w.LPVOID,
            w.UINT,
        ]
        self._user32.SystemParametersInfoW.restype = w.BOOL
        self._set_thread_dpi_awareness = getattr(
            self._user32, "SetThreadDpiAwarenessContext", None
        )
        if self._set_thread_dpi_awareness is not None:
            self._set_thread_dpi_awareness.argtypes = [w.HANDLE]
            self._set_thread_dpi_awareness.restype = w.HANDLE
        self._user32.GetWindowRgn.argtypes = [w.HWND, w.HANDLE]
        self._user32.GetWindowRgn.restype = ctypes.c_int
        self._user32.GetLayeredWindowAttributes.argtypes = [
            w.HWND,
            ctypes.POINTER(w.DWORD),
            ctypes.POINTER(w.BYTE),
            ctypes.POINTER(w.DWORD),
        ]
        self._user32.GetLayeredWindowAttributes.restype = w.BOOL
        self._get_window_long = getattr(
            self._user32, "GetWindowLongPtrW", self._user32.GetWindowLongW
        )
        self._get_window_long.argtypes = [w.HWND, ctypes.c_int]
        self._get_window_long.restype = ctypes.c_ssize_t
        self._get_dpi_for_window = getattr(self._user32, "GetDpiForWindow", None)
        if self._get_dpi_for_window is not None:
            self._get_dpi_for_window.argtypes = [w.HWND]
            self._get_dpi_for_window.restype = w.UINT

        self._gdi32.CreateRectRgn.argtypes = [
            ctypes.c_int,
            ctypes.c_int,
            ctypes.c_int,
            ctypes.c_int,
        ]
        self._gdi32.CreateRectRgn.restype = w.HANDLE
        self._gdi32.GetRgnBox.argtypes = [w.HANDLE, rect_pointer]
        self._gdi32.GetRgnBox.restype = ctypes.c_int
        self._gdi32.PtInRegion.argtypes = [w.HANDLE, ctypes.c_int, ctypes.c_int]
        self._gdi32.PtInRegion.restype = w.BOOL
        self._gdi32.DeleteObject.argtypes = [w.HANDLE]
        self._gdi32.DeleteObject.restype = w.BOOL

        self._dwmapi.DwmGetWindowAttribute.argtypes = [
            w.HWND,
            w.DWORD,
            w.LPVOID,
            w.DWORD,
        ]
        self._dwmapi.DwmGetWindowAttribute.restype = w.LONG

    @staticmethod
    def _handle_value(handle: Any) -> int:
        if handle is None:
            return 0
        value = getattr(handle, "value", handle)
        return int(value or 0)

    @classmethod
    def _handle_hex(cls, handle: Any) -> str:
        return f"0x{cls._handle_value(handle):x}"

    @staticmethod
    def _rect_dict(bounds: Any) -> dict[str, int]:
        return {
            "left": int(bounds.left),
            "top": int(bounds.top),
            "right": int(bounds.right),
            "bottom": int(bounds.bottom),
        }

    @staticmethod
    def _hresult_hex(result: int) -> str:
        return f"0x{result & 0xFFFFFFFF:08x}"

    @staticmethod
    def _last_error_details() -> dict[str, Any]:
        code = ctypes.get_last_error()
        return {
            "win32_error": code,
            "win32_message": ctypes.FormatError(code).strip()
            if code
            else "unknown error",
        }

    def _measure_monitor_dpis(
        self, monitors: Sequence[tuple[int, Mapping[str, int]]]
    ) -> dict[int, dict[str, Any]]:
        if self._set_thread_dpi_awareness is None or self._get_dpi_for_window is None:
            unavailable = _unavailable(
                "private_pmv2_probe_unavailable",
                "PMv2 thread awareness or GetDpiForWindow is unavailable",
            )
            return {handle: unavailable for handle, _bounds in monitors}

        pmv2_context = self._wintypes.HANDLE(
            ctypes.c_void_p(self._DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).value
        )
        ctypes.set_last_error(0)
        previous_context = self._set_thread_dpi_awareness(pmv2_context)
        if not self._handle_value(previous_context):
            unavailable = _unavailable(
                "private_pmv2_probe_context_failed",
                "SetThreadDpiAwarenessContext(PMv2) failed",
                **self._last_error_details(),
            )
            return {handle: unavailable for handle, _bounds in monitors}

        probe = None
        result: dict[int, dict[str, Any]] = {}
        cleanup_failure: ProbeFailure | None = None
        try:
            ctypes.set_last_error(0)
            probe = self._user32.CreateWindowExW(
                self._WS_EX_TOOLWINDOW | self._WS_EX_NOACTIVATE,
                "STATIC",
                "",
                self._WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                None,
                None,
            )
            if not self._handle_value(probe):
                unavailable = _unavailable(
                    "private_pmv2_probe_create_failed",
                    "CreateWindowExW could not create the private hidden DPI probe",
                    **self._last_error_details(),
                )
                result = {handle: unavailable for handle, _bounds in monitors}
            elif self._user32.IsWindowVisible(probe):
                unavailable = _unavailable(
                    "private_pmv2_probe_visibility_violation",
                    "the private DPI probe unexpectedly became visible",
                )
                result = {handle: unavailable for handle, _bounds in monitors}
            else:
                for handle, bounds in monitors:
                    x = (
                        int(bounds["left"])
                        + (int(bounds["right"]) - int(bounds["left"])) // 2
                    )
                    y = (
                        int(bounds["top"])
                        + (int(bounds["bottom"]) - int(bounds["top"])) // 2
                    )
                    ctypes.set_last_error(0)
                    moved = self._user32.SetWindowPos(
                        probe,
                        None,
                        x,
                        y,
                        0,
                        0,
                        self._SWP_NOSIZE | self._SWP_NOZORDER | self._SWP_NOACTIVATE,
                    )
                    if not moved:
                        result[handle] = _unavailable(
                            "private_pmv2_probe_position_failed",
                            "SetWindowPos failed for the private hidden DPI probe",
                            hmonitor=self._handle_hex(handle),
                            **self._last_error_details(),
                        )
                        continue
                    if self._user32.IsWindowVisible(probe):
                        result[handle] = _unavailable(
                            "private_pmv2_probe_visibility_violation",
                            "the private DPI probe unexpectedly became visible",
                            hmonitor=self._handle_hex(handle),
                        )
                        continue
                    observed_monitor = self._user32.MonitorFromWindow(
                        probe, self._MONITOR_DEFAULTTONEAREST
                    )
                    binding = evaluate_private_probe_binding(
                        self._handle_hex(handle), self._handle_hex(observed_monitor)
                    )
                    if binding.get("available") is not True:
                        result[handle] = binding
                        continue
                    dpi = int(self._get_dpi_for_window(probe))
                    if dpi <= 0:
                        result[handle] = _unavailable(
                            "private_pmv2_probe_dpi_failed",
                            "GetDpiForWindow returned zero for the private hidden DPI probe",
                            hmonitor=self._handle_hex(handle),
                            **self._last_error_details(),
                        )
                        continue
                    result[handle] = {
                        "available": True,
                        "x": dpi,
                        "y": dpi,
                        "source": "GetDpiForWindow(private_hidden_pmv2_probe)",
                        "authority": "private_hidden_pmv2_probe",
                        "probe": {
                            "scope": "probe_owned_hidden_hwnd",
                            "visible": False,
                            "activated": False,
                            "observed_windows_moved": False,
                            "monitor_binding": binding,
                        },
                    }
        finally:
            if self._handle_value(probe):
                ctypes.set_last_error(0)
                if not self._user32.DestroyWindow(probe):
                    cleanup_failure = ProbeFailure(
                        "private_pmv2_probe_destroy_failed",
                        "DestroyWindow failed for the private hidden DPI probe",
                        probe_hwnd=self._handle_hex(probe),
                        **self._last_error_details(),
                    )
            ctypes.set_last_error(0)
            if not self._set_thread_dpi_awareness(previous_context):
                cleanup_failure = ProbeFailure(
                    "dpi_awareness_context_restore_failed",
                    "the previous thread DPI awareness context could not be restored",
                    **self._last_error_details(),
                )
        if cleanup_failure is not None:
            raise cleanup_failure
        for measurement in result.values():
            probe_evidence = measurement.get("probe")
            if isinstance(probe_evidence, dict):
                probe_evidence["destroyed_after_topology"] = True
                probe_evidence["thread_dpi_context_restored"] = True
        return result

    def _monitor_dpi(self, monitor: Any) -> dict[str, Any]:
        return self._monitor_dpi_cache.get(
            self._handle_value(monitor),
            _unavailable(
                "monitor_effective_dpi_not_measured",
                "the HMONITOR was not present in the PMv2 probe topology snapshot",
                hmonitor=self._handle_hex(monitor),
            ),
        )

    def enumerate_monitors(self) -> list[dict[str, Any]]:
        monitors: list[dict[str, Any]] = []
        raw_monitors: list[tuple[int, Mapping[str, int]]] = []
        failure: list[ProbeFailure] = []

        @self._monitor_callback_type
        def callback(monitor: Any, _device: Any, _bounds: Any, _data: int) -> int:
            info = self._MonitorInfoEx()
            info.cbSize = ctypes.sizeof(self._MonitorInfoEx)
            if not self._user32.GetMonitorInfoW(monitor, ctypes.byref(info)):
                failure.append(
                    ProbeFailure(
                        "monitor_info_query_failed",
                        "GetMonitorInfoW failed during active-monitor enumeration",
                        hmonitor=self._handle_hex(monitor),
                        **self._last_error_details(),
                    )
                )
                return 0
            monitors.append(
                {
                    "hmonitor": self._handle_hex(monitor),
                    "active": True,
                    "device_name": str(info.szDevice),
                    "primary": bool(info.dwFlags & self._MONITORINFOF_PRIMARY),
                    "bounds": self._rect_dict(info.rcMonitor),
                    "work_area": self._rect_dict(info.rcWork),
                }
            )
            raw_monitors.append(
                (self._handle_value(monitor), self._rect_dict(info.rcMonitor))
            )
            return 1

        ctypes.set_last_error(0)
        succeeded = self._user32.EnumDisplayMonitors(None, None, callback, 0)
        if failure:
            raise failure[0]
        if not succeeded:
            raise ProbeFailure(
                "monitor_enumeration_failed",
                "EnumDisplayMonitors failed",
                **self._last_error_details(),
            )
        self._monitor_dpi_cache = self._measure_monitor_dpis(raw_monitors)
        self._monitor_geometry_cache = {
            int(str(monitor["hmonitor"]), 16): {
                "bounds": dict(monitor["bounds"]),
                "work_area": dict(monitor["work_area"]),
            }
            for monitor in monitors
        }
        for monitor in monitors:
            handle = int(str(monitor["hmonitor"]), 16)
            monitor["effective_dpi"] = self._monitor_dpi_cache[handle]
        monitors.sort(
            key=lambda item: (
                int(item["bounds"]["left"]),
                int(item["bounds"]["top"]),
                str(item["hmonitor"]),
            )
        )
        return monitors

    def _enumerate_windows(self) -> list[int]:
        handles: list[int] = []

        @self._window_callback_type
        def callback(window: Any, _data: int) -> int:
            handles.append(self._handle_value(window))
            return 1

        ctypes.set_last_error(0)
        if not self._user32.EnumWindows(callback, 0):
            raise ProbeFailure(
                "window_enumeration_failed",
                "EnumWindows failed",
                **self._last_error_details(),
            )
        return handles

    def _class_name(self, window: Any) -> dict[str, Any]:
        buffer = ctypes.create_unicode_buffer(256)
        ctypes.set_last_error(0)
        copied = self._user32.GetClassNameW(window, buffer, len(buffer))
        if copied <= 0:
            return _unavailable(
                "window_class_query_failed",
                "GetClassNameW failed",
                **self._last_error_details(),
            )
        return _available(buffer.value, "GetClassNameW")

    def _window_rect(self, window: Any) -> dict[str, Any]:
        bounds = self._Rect()
        ctypes.set_last_error(0)
        if not self._user32.GetWindowRect(window, ctypes.byref(bounds)):
            return _unavailable(
                "window_rect_query_failed",
                "GetWindowRect failed",
                **self._last_error_details(),
            )
        return _available(self._rect_dict(bounds), "GetWindowRect")

    def _dwm_bounds(self, window: Any) -> dict[str, Any]:
        bounds = self._Rect()
        result = self._dwmapi.DwmGetWindowAttribute(
            window,
            self._DWMWA_EXTENDED_FRAME_BOUNDS,
            ctypes.byref(bounds),
            ctypes.sizeof(bounds),
        )
        if result != 0:
            return _unavailable(
                "dwm_extended_frame_bounds_query_failed",
                "DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS) failed",
                hresult=self._hresult_hex(result),
            )
        return _available(
            self._rect_dict(bounds),
            "DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)",
        )

    def _window_dpi(self, window: Any) -> dict[str, Any]:
        if self._get_dpi_for_window is None:
            return _unavailable(
                "get_dpi_for_window_unavailable",
                "GetDpiForWindow is unavailable on this system",
            )
        dpi = int(self._get_dpi_for_window(window))
        if dpi <= 0:
            return _unavailable(
                "window_dpi_query_failed",
                "GetDpiForWindow returned zero",
                **self._last_error_details(),
            )
        return _available(dpi, "GetDpiForWindow")

    def _window_process(self, window: Any) -> tuple[dict[str, Any], dict[str, Any]]:
        process_id = self._wintypes.DWORD()
        ctypes.set_last_error(0)
        thread_id = int(
            self._user32.GetWindowThreadProcessId(window, ctypes.byref(process_id))
        )
        if thread_id == 0:
            reason = _unavailable(
                "window_process_query_failed",
                "GetWindowThreadProcessId failed",
                **self._last_error_details(),
            )
            return reason, reason
        return (
            _available(int(process_id.value), "GetWindowThreadProcessId"),
            _available(thread_id, "GetWindowThreadProcessId"),
        )

    def _scan_region_edge(self, region: Any, bounds: Any, edge: str) -> dict[str, Any]:
        width = int(bounds.right) + int(bounds.left)
        height = int(bounds.bottom) + int(bounds.top)
        if width <= 0 or height <= 0:
            return _unavailable(
                "region_dimensions_unavailable",
                "region bounds could not establish positive symmetric dimensions",
                edge=edge,
            )
        if edge in {"top", "bottom"}:
            maximum_depth = height // 2
        else:
            maximum_depth = width // 2
        outer_inset: int | None = None
        inner_inset: int | None = None
        probe_points = 0
        for depth in range(maximum_depth + 1):
            if edge == "top":
                x, y = width // 2, depth
            elif edge == "bottom":
                x, y = width // 2, height - 1 - depth
            elif edge == "left":
                x, y = depth, height // 2
            else:
                x, y = width - 1 - depth, height // 2
            probe_points += 1
            inside = bool(self._gdi32.PtInRegion(region, x, y))
            if outer_inset is None and inside:
                outer_inset = depth
            elif outer_inset is not None and not inside:
                inner_inset = depth
                break
        if outer_inset is None:
            return _unavailable(
                "edge_region_band_unavailable",
                "PtInRegion found no included pixel on the edge midline",
                edge=edge,
                probe_points=probe_points,
            )
        if inner_inset is None:
            return _unavailable(
                "inner_cutout_not_observed",
                "PtInRegion did not find an inner hole before the region center",
                edge=edge,
                outer_inset_px=outer_inset,
                probe_points=probe_points,
            )
        return {
            "available": True,
            "outer_inset_px": outer_inset,
            "inner_inset_px": inner_inset,
            "probe_points": probe_points,
            "source": f"PtInRegion({edge}_center_scan)",
        }

    def _window_region(self, window: Any) -> dict[str, Any]:
        region = self._gdi32.CreateRectRgn(0, 0, 0, 0)
        if not self._handle_value(region):
            return _unavailable(
                "region_buffer_allocation_failed",
                "CreateRectRgn failed while allocating a readback buffer",
                **self._last_error_details(),
            )
        try:
            ctypes.set_last_error(0)
            complexity = int(self._user32.GetWindowRgn(window, region))
            if complexity == 0:
                return _unavailable(
                    "window_region_query_failed",
                    "GetWindowRgn failed",
                    **self._last_error_details(),
                )
            name = self._REGION_COMPLEXITY.get(complexity)
            if name is None:
                return _unavailable(
                    "window_region_complexity_unknown",
                    "GetWindowRgn returned an unknown region complexity",
                    observed=complexity,
                )
            bounds_value: dict[str, int] | None = None
            edge_bands: dict[str, dict[str, Any]]
            point_membership: dict[str, bool] | None
            if name != "null":
                bounds = self._Rect()
                box_complexity = int(
                    self._gdi32.GetRgnBox(region, ctypes.byref(bounds))
                )
                if box_complexity == 0:
                    return _unavailable(
                        "window_region_bounds_query_failed",
                        "GetRgnBox failed",
                        **self._last_error_details(),
                    )
                bounds_value = self._rect_dict(bounds)
                edge_bands = {
                    edge: self._scan_region_edge(region, bounds, edge)
                    for edge in ("top", "bottom", "left", "right")
                }
                width = int(bounds.right) + int(bounds.left)
                height = int(bounds.bottom) + int(bounds.top)
                points = {
                    "top_left_corner": (0, 0),
                    "top_right_corner": (width - 1, 0),
                    "bottom_left_corner": (0, height - 1),
                    "bottom_right_corner": (width - 1, height - 1),
                    "top_midpoint": (width // 2, 0),
                    "bottom_midpoint": (width // 2, height - 1),
                    "left_midpoint": (0, height // 2),
                    "right_midpoint": (width - 1, height // 2),
                    "center": (width // 2, height // 2),
                }
                point_membership = {
                    label: bool(self._gdi32.PtInRegion(region, x, y))
                    for label, (x, y) in points.items()
                }
            else:
                edge_bands = {
                    edge: _unavailable(
                        "null_window_region",
                        "a null window region has no outer-to-inner band",
                        edge=edge,
                    )
                    for edge in ("top", "bottom", "left", "right")
                }
                point_membership = None
            return {
                "available": True,
                "complexity": name,
                "bounds": bounds_value,
                "edge_bands": edge_bands,
                "point_membership": point_membership,
                "source": "GetWindowRgn/GetRgnBox",
            }
        finally:
            self._gdi32.DeleteObject(region)

    def _layered_state(self, window: Any) -> dict[str, Any]:
        ctypes.set_last_error(0)
        ex_style = int(self._get_window_long(window, self._GWL_EXSTYLE))
        error = ctypes.get_last_error()
        if ex_style == 0 and error:
            return _unavailable(
                "extended_style_query_failed",
                "GetWindowLongPtrW(GWL_EXSTYLE) failed",
                **self._last_error_details(),
            )
        extended_styles = _available(
            {
                "topmost": bool(ex_style & self._WS_EX_TOPMOST),
                "no_activate": bool(ex_style & self._WS_EX_NOACTIVATE),
                "tool_window": bool(ex_style & self._WS_EX_TOOLWINDOW),
                "transparent": bool(ex_style & self._WS_EX_TRANSPARENT),
            },
            "GetWindowLongPtrW(GWL_EXSTYLE)",
        )
        layered = bool(ex_style & self._WS_EX_LAYERED)
        if not layered:
            alpha = _unavailable(
                "window_not_layered",
                "layered alpha is undefined for a non-layered window",
            )
            return {
                "available": True,
                "value": False,
                "ex_style_hex": f"0x{ex_style:x}",
                "extended_styles": extended_styles,
                "alpha": alpha,
                "source": "GetWindowLongPtrW(GWL_EXSTYLE)",
            }
        color_key = self._wintypes.DWORD()
        alpha_value = self._wintypes.BYTE()
        flags = self._wintypes.DWORD()
        ctypes.set_last_error(0)
        if self._user32.GetLayeredWindowAttributes(
            window,
            ctypes.byref(color_key),
            ctypes.byref(alpha_value),
            ctypes.byref(flags),
        ):
            alpha = _available(int(alpha_value.value), "GetLayeredWindowAttributes")
            flags_value: dict[str, Any] = _available(
                int(flags.value), "GetLayeredWindowAttributes"
            )
        else:
            details = self._last_error_details()
            alpha = _unavailable(
                "layered_alpha_query_failed",
                "GetLayeredWindowAttributes failed",
                **details,
            )
            flags_value = _unavailable(
                "layered_flags_query_failed",
                "GetLayeredWindowAttributes failed",
                **details,
            )
        return {
            "available": True,
            "value": True,
            "ex_style_hex": f"0x{ex_style:x}",
            "extended_styles": extended_styles,
            "alpha": alpha,
            "flags": flags_value,
            "source": "GetWindowLongPtrW/GetLayeredWindowAttributes",
        }

    def _display_affinity(self, window: Any) -> dict[str, Any]:
        affinity = self._wintypes.DWORD()
        ctypes.set_last_error(0)
        if not self._user32.GetWindowDisplayAffinity(window, ctypes.byref(affinity)):
            return _unavailable(
                "display_affinity_query_failed",
                "GetWindowDisplayAffinity failed; foreign-process visibility may be restricted",
                **self._last_error_details(),
            )
        return _available(int(affinity.value), "GetWindowDisplayAffinity")

    def system_animation_enabled(self) -> dict[str, Any]:
        enabled = self._wintypes.BOOL(1)
        ctypes.set_last_error(0)
        if not self._user32.SystemParametersInfoW(
            self._SPI_GETCLIENTAREAANIMATION,
            0,
            ctypes.byref(enabled),
            0,
        ):
            return _unavailable(
                "system_animation_preference_query_failed",
                "SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION) failed",
                **self._last_error_details(),
            )
        return _available(
            bool(enabled.value),
            "SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION)",
        )

    def _z_order(
        self, window: Any, all_windows: Sequence[int], target_hwnd: int
    ) -> dict[str, Any]:
        window_value = self._handle_value(window)
        try:
            window_index = all_windows.index(window_value)
            target_index = all_windows.index(target_hwnd)
        except ValueError:
            return _unavailable(
                "desktop_z_order_unavailable",
                "target or indicator HWND was absent from EnumWindows",
            )
        if window_index < target_index:
            relation = "above"
        elif window_index > target_index:
            relation = "below"
        else:
            relation = "same"
        previous = self._user32.GetWindow(window, self._GW_HWNDPREV)
        following = self._user32.GetWindow(window, self._GW_HWNDNEXT)
        return {
            "available": True,
            "desktop_index_top_to_bottom": window_index,
            "target_index_top_to_bottom": target_index,
            "relation_to_target": relation,
            "previous_hwnd": (
                self._handle_hex(previous) if self._handle_value(previous) else None
            ),
            "next_hwnd": (
                self._handle_hex(following) if self._handle_value(following) else None
            ),
            "source": "EnumWindows/GetWindow",
        }

    def _inspect_target(self, target_pid: int, target_hwnd: int) -> dict[str, Any]:
        window = self._wintypes.HWND(target_hwnd)
        is_window = bool(self._user32.IsWindow(window))
        process_id, _thread_id = (
            self._window_process(window)
            if is_window
            else (
                _unavailable(
                    "target_hwnd_invalid", "the target HWND is not a live window"
                ),
                _unavailable(
                    "target_hwnd_invalid", "the target HWND is not a live window"
                ),
            )
        )
        monitor = (
            self._user32.MonitorFromWindow(window, self._MONITOR_DEFAULTTONEAREST)
            if is_window
            else None
        )
        monitor_handle = (
            _available(self._handle_hex(monitor), "MonitorFromWindow")
            if self._handle_value(monitor)
            else _unavailable(
                "target_monitor_query_failed", "MonitorFromWindow returned no monitor"
            )
        )
        monitor_dpi = (
            self._monitor_dpi(monitor)
            if self._handle_value(monitor)
            else _unavailable(
                "target_monitor_effective_dpi_unavailable",
                "the target monitor could not be resolved",
            )
        )
        if monitor_dpi.get("available") is True:
            if int(monitor_dpi["x"]) == int(monitor_dpi["y"]):
                effective_dpi = {
                    "available": True,
                    "value": int(monitor_dpi["x"]),
                    "source": "GetDpiForWindow(private_hidden_pmv2_probe)",
                    "authority": "private_hidden_pmv2_probe",
                }
            else:
                effective_dpi = _unavailable(
                    "asymmetric_effective_dpi_unsupported",
                    "target monitor effective DPI differs between axes",
                    x=monitor_dpi["x"],
                    y=monitor_dpi["y"],
                )
        else:
            effective_dpi = monitor_dpi
        monitor_geometry = self._monitor_geometry_cache.get(self._handle_value(monitor))
        monitor_bounds = (
            _available(
                dict(monitor_geometry["bounds"]),
                "GetMonitorInfoW(topology_snapshot)",
            )
            if monitor_geometry is not None
            else _unavailable(
                "target_monitor_geometry_not_measured",
                "the target HMONITOR was absent from the topology geometry cache",
            )
        )
        monitor_work_area = (
            _available(
                dict(monitor_geometry["work_area"]),
                "GetMonitorInfoW(topology_snapshot)",
            )
            if monitor_geometry is not None
            else _unavailable(
                "target_monitor_work_area_not_measured",
                "the target HMONITOR work area was absent from the topology cache",
            )
        )
        return {
            "requested_pid": target_pid,
            "requested_hwnd": self._handle_hex(target_hwnd),
            "is_window": _available(is_window, "IsWindow"),
            "process_id": process_id,
            "window_rect": (
                self._window_rect(window)
                if is_window
                else _unavailable("target_hwnd_invalid", "the target HWND is not live")
            ),
            "dwm_extended_frame_bounds": (
                self._dwm_bounds(window)
                if is_window
                else _unavailable("target_hwnd_invalid", "the target HWND is not live")
            ),
            "window_dpi": (
                self._window_dpi(window)
                if is_window
                else _unavailable("target_hwnd_invalid", "the target HWND is not live")
            ),
            "monitor_handle": monitor_handle,
            "monitor_bounds": monitor_bounds,
            "monitor_work_area": monitor_work_area,
            "monitor_effective_dpi": effective_dpi,
        }

    def _inspect_indicator(
        self,
        hwnd: int,
        class_name: str,
        all_windows: Sequence[int],
        target_hwnd: int,
    ) -> dict[str, Any]:
        window = self._wintypes.HWND(hwnd)
        process_id, thread_id = self._window_process(window)
        monitor = self._user32.MonitorFromWindow(window, self._MONITOR_DEFAULTTONEAREST)
        if class_name == self._BANNER_CLASS:
            logical_role = _available("control_banner", "exact_window_class")
        else:
            logical_role = _unavailable(
                "logical_role_not_exposed_by_win32",
                "DccCuaControlFrame is shared by frame bands and the hidden DPI probe",
                candidates=["control_frame", "dpi_probe"],
            )
        layered = self._layered_state(window)
        extended_styles = (
            layered.get("extended_styles")
            if isinstance(layered, Mapping)
            else _unavailable(
                "extended_style_query_failed",
                "layered state did not expose extended window styles",
            )
        )
        owner = self._user32.GetWindow(window, self._GW_OWNER)
        return {
            "hwnd": self._handle_hex(hwnd),
            "class_name": class_name,
            "logical_role": logical_role,
            "process_id": process_id,
            "thread_id": thread_id,
            "visible": _available(
                bool(self._user32.IsWindowVisible(window)), "IsWindowVisible"
            ),
            "window_rect": self._window_rect(window),
            "dwm_extended_frame_bounds": self._dwm_bounds(window),
            "dpi": self._window_dpi(window),
            "monitor_handle": (
                _available(self._handle_hex(monitor), "MonitorFromWindow")
                if self._handle_value(monitor)
                else _unavailable(
                    "indicator_monitor_query_failed",
                    "MonitorFromWindow returned no monitor",
                )
            ),
            "region": self._window_region(window),
            "layered": layered,
            "extended_styles": extended_styles,
            "owner_hwnd": _available(
                self._handle_hex(owner) if self._handle_value(owner) else None,
                "GetWindow(GW_OWNER)",
            ),
            "display_affinity": self._display_affinity(window),
            "z_order": self._z_order(window, all_windows, target_hwnd),
        }

    def observe(self, target_pid: int, target_hwnd: int) -> dict[str, Any]:
        all_windows = self._enumerate_windows()
        indicators: list[dict[str, Any]] = []
        for hwnd in all_windows:
            window = self._wintypes.HWND(hwnd)
            class_name_field = self._class_name(window)
            class_name = _available_value(class_name_field)
            if class_name not in {self._BANNER_CLASS, self._FRAME_CLASS}:
                continue
            indicators.append(
                self._inspect_indicator(
                    hwnd,
                    str(class_name),
                    all_windows,
                    target_hwnd,
                )
            )
        return {
            "target": self._inspect_target(target_pid, target_hwnd),
            "indicator_windows": indicators,
        }


def _sampling_offsets(duration_ms: int, interval_ms: int) -> tuple[int, ...]:
    offsets = list(range(0, duration_ms + 1, interval_ms))
    if offsets[-1] != duration_ms:
        offsets.append(duration_ms)
    return tuple(offsets)


def run_probe(
    source: ObservationSource,
    config: ProbeConfig,
    contract: FrameContract,
    emit: Callable[[dict[str, Any]], None],
    *,
    sleep: Callable[[float], None],
    timestamp: Callable[[], str],
    monotonic: Callable[[], float] = time.monotonic,
) -> ProbeRunResult:
    """Sample through the injected read-only seam and emit one JSON object per event."""

    monitors = list(source.enumerate_monitors())
    topology = evaluate_topology(monitors, contract)
    motion_resolution = resolve_motion(source, config.motion_mode)
    emit(
        {
            "schema": SCHEMA,
            "event": "topology_summary",
            "observation_only": True,
            "observation_scope": {
                "target_or_indicator_window_mutations": [],
                "input_injection": False,
                "private_dpi_probe": (
                    "one never-shown PMv2 HWND is created, repositioned with "
                    "SWP_NOACTIVATE, verified, destroyed, and its thread context restored"
                ),
            },
            "observed_at": timestamp(),
            "monitors": monitors,
            "motion_resolution": motion_resolution,
            **topology,
        }
    )
    if not topology["eligible"]:
        cleanup = {
            "schema": SCHEMA,
            "event": "cleanup_summary",
            "observation_only": True,
            "observed_at": timestamp(),
            "status": "not_started_topology_ineligible",
            "cleanup_required": config.require_cleanup,
            "convergence_observed": False,
            "ever_seen_count": 0,
            "removed_hwnds": [],
            "remaining_hwnds": [],
            "accepted": False,
        }
        emit(cleanup)
        return ProbeRunResult(
            exit_code=2,
            converged=False,
            cleanup_status=cleanup["status"],
        )

    ever_seen: set[str] = set()
    final_seen: set[str] = set()
    converged = False
    final_converged = False
    teardown_started = False
    lifecycle_reasons: list[dict[str, Any]] = []
    lifecycle_reason_kinds: set[str] = set()
    motion_samples: list[dict[str, int]] = []
    migration_samples: list[dict[str, Any]] = []
    started = monotonic()
    for sequence, offset in enumerate(
        _sampling_offsets(config.duration_ms, config.interval_ms)
    ):
        if sequence:
            remaining = started + offset / 1_000 - monotonic()
            if remaining > 0:
                sleep(remaining)
        collection_started = monotonic()
        snapshot = source.observe(config.target_pid, config.target_hwnd)
        collection_finished = monotonic()
        observed_elapsed_ms = round(
            ((collection_started + collection_finished) / 2 - started) * 1_000
        )
        target = snapshot.get("target")
        windows = snapshot.get("indicator_windows")
        if not isinstance(target, Mapping) or not isinstance(windows, Sequence):
            raise TypeError("observation source returned an invalid snapshot")
        indicator_windows = list(windows)
        evaluation = evaluate_sample(target, indicator_windows, contract)
        sample_converged = bool(evaluation["converged"])
        previously_converged = converged
        converged |= sample_converged
        final_converged = sample_converged
        if config.require_cleanup and previously_converged:
            if teardown_started and indicator_windows:
                kind = "indicator_windows_reappeared_after_teardown"
                if kind not in lifecycle_reason_kinds:
                    lifecycle_reason_kinds.add(kind)
                    lifecycle_reasons.append(
                        _reason(
                            kind,
                            "indicator HWNDs reappeared after the all-absent teardown phase began",
                            observed_hwnds=[
                                window.get("hwnd") for window in indicator_windows
                            ],
                        )
                    )
            elif not teardown_started and not sample_converged:
                if indicator_windows:
                    kind = "partial_indicator_teardown_observed"
                    if kind not in lifecycle_reason_kinds:
                        lifecycle_reason_kinds.add(kind)
                        lifecycle_reasons.append(
                            _reason(
                                kind,
                                "cleanup exposed a partial or broken indicator family before all HWNDs disappeared",
                                observed_hwnds=[
                                    window.get("hwnd") for window in indicator_windows
                                ],
                            )
                        )
                else:
                    teardown_started = True
        outer_alpha = evaluation["frame_contract"].get("outer_band_alpha")
        if isinstance(outer_alpha, int):
            motion_samples.append(
                {
                    "elapsed_ms": observed_elapsed_ms,
                    "outer_band_alpha": outer_alpha,
                }
            )
        migration_samples.append(
            {
                "elapsed_ms": observed_elapsed_ms,
                "converged": sample_converged,
                "monitor_handle": _available_value(target.get("monitor_handle")),
                "monitor_dpi": _available_value(target.get("monitor_effective_dpi")),
                "dwm_bounds": _available_value(target.get("dwm_extended_frame_bounds")),
            }
        )
        final_seen = {
            str(window.get("hwnd"))
            for window in indicator_windows
            if window.get("hwnd") is not None
        }
        ever_seen.update(final_seen)
        emit(
            {
                "schema": SCHEMA,
                "event": "sample",
                "observation_only": True,
                "observed_at": timestamp(),
                "sequence": sequence,
                "scheduled_elapsed_ms": offset,
                "observed_elapsed_ms": observed_elapsed_ms,
                "collection_duration_ms": round(
                    (collection_finished - collection_started) * 1_000
                ),
                "target": target,
                "indicator_windows": indicator_windows,
                "evaluation": evaluation,
            }
        )

    motion_evaluation = evaluate_motion_samples(
        motion_samples,
        contract,
        motion_resolution,
        interval_ms=config.interval_ms,
    )
    emit(
        {
            "schema": SCHEMA,
            "event": "motion_summary",
            "observation_only": True,
            "observed_at": timestamp(),
            **motion_evaluation,
        }
    )
    migration_evaluation = evaluate_migration_samples(
        migration_samples,
        monitors,
        stable_samples_required=2,
    )
    emit(
        {
            "schema": SCHEMA,
            "event": "migration_summary",
            "observation_only": True,
            "observed_at": timestamp(),
            **migration_evaluation,
        }
    )
    removed = sorted(ever_seen - final_seen)
    remaining = sorted(final_seen)
    if ever_seen and not remaining:
        cleanup_status = "cleaned"
    elif remaining:
        cleanup_status = "remaining"
    else:
        cleanup_status = "not_observed"
    if config.require_cleanup:
        if not teardown_started:
            lifecycle_reasons.append(
                _reason(
                    "cleanup_all_absent_teardown_not_observed",
                    "cleanup requires an all-absent sample after convergence",
                )
            )
        if remaining:
            lifecycle_reasons.append(
                _reason(
                    "cleanup_final_windows_remaining",
                    "cleanup requires the final sample to contain no indicator HWNDs",
                    remaining_hwnds=remaining,
                )
            )
        lifecycle_accepted = (
            teardown_started and not remaining and not lifecycle_reasons
        )
    else:
        if not final_converged:
            lifecycle_reasons.append(
                _reason(
                    "steady_state_final_sample_not_converged",
                    "steady-state acceptance requires the final sample to remain converged",
                )
            )
        lifecycle_accepted = final_converged and not lifecycle_reasons
    accepted = (
        converged
        and motion_evaluation["accepted"]
        and migration_evaluation["accepted"]
        and lifecycle_accepted
    )
    emit(
        {
            "schema": SCHEMA,
            "event": "cleanup_summary",
            "observation_only": True,
            "observed_at": timestamp(),
            "status": cleanup_status,
            "cleanup_required": config.require_cleanup,
            "convergence_observed": converged,
            "final_sample_converged": final_converged,
            "all_absent_teardown_observed": teardown_started,
            "motion_accepted": motion_evaluation["accepted"],
            "migration_accepted": migration_evaluation["accepted"],
            "ever_seen_count": len(ever_seen),
            "removed_hwnds": removed,
            "remaining_hwnds": remaining,
            "blocking_reasons": lifecycle_reasons,
            "accepted": accepted,
        }
    )
    return ProbeRunResult(
        exit_code=0 if accepted else 4,
        converged=converged,
        cleanup_status=cleanup_status,
    )


def _parse_handle(value: str) -> int:
    try:
        parsed = int(value, 0)
    except ValueError as error:
        raise argparse.ArgumentTypeError(
            "HWND must be decimal or 0x-prefixed"
        ) from error
    if parsed <= 0:
        raise argparse.ArgumentTypeError("HWND must be positive")
    return parsed


def _load_contract(path: Path) -> FrameContract:
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
        frame = document["indicator"]["frame"]
        contract = FrameContract(
            thickness_dip=int(frame["thickness_dip"]),
            gradient_steps=int(frame["gradient_steps"]),
            alpha_max=int(frame["alpha_max"]),
            alpha_min=int(frame["alpha_min"]),
            pulse_period_ms=int(frame["pulse_period_ms"]),
        )
    except (OSError, ValueError, KeyError, TypeError, json.JSONDecodeError) as error:
        raise ProbeFailure(
            "theme_contract_unavailable",
            "the indicator frame contract could not be loaded",
            path=str(path),
            error=str(error),
        ) from error
    if contract.thickness_dip <= 0 or contract.gradient_steps <= 0:
        raise ProbeFailure(
            "theme_contract_invalid",
            "frame thickness and gradient step count must be positive",
            path=str(path),
        )
    if not 0 <= contract.alpha_max <= 255:
        raise ProbeFailure(
            "theme_contract_invalid",
            "frame alpha_max must be between 0 and 255",
            path=str(path),
        )
    if not 0 <= contract.alpha_min <= contract.alpha_max:
        raise ProbeFailure(
            "theme_contract_invalid",
            "frame alpha_min must be between zero and alpha_max",
            path=str(path),
        )
    if contract.pulse_period_ms <= 0:
        raise ProbeFailure(
            "theme_contract_invalid",
            "frame pulse_period_ms must be positive",
            path=str(path),
        )
    if contract != ACCEPTANCE_FRAME_CONTRACT:
        fields = (
            "thickness_dip",
            "gradient_steps",
            "alpha_min",
            "alpha_max",
            "pulse_period_ms",
        )
        raise ProbeFailure(
            "theme_contract_acceptance_mismatch",
            "the mutable theme does not match the fixed indicator acceptance contract",
            path=str(path),
            expected={
                field: getattr(ACCEPTANCE_FRAME_CONTRACT, field) for field in fields
            },
            observed={field: getattr(contract, field) for field in fields},
        )
    return ACCEPTANCE_FRAME_CONTRACT


def _utc_timestamp() -> str:
    return datetime.now(UTC).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def _argument_parser() -> argparse.ArgumentParser:
    default_theme = (
        Path(__file__).resolve().parents[1]
        / "crates"
        / "dcc-cua-indicator"
        / "theme"
        / "dcc-cua-theme.json"
    )
    parser = argparse.ArgumentParser(
        description=(
            "Observe the mixed-DPI DCC CUA indicator without activating, moving, or "
            "inputting into target/indicator windows. Only a private invisible PMv2 "
            "probe HWND is repositioned. Output is JSONL on stdout."
        )
    )
    parser.add_argument("--target-pid", type=int, required=True)
    parser.add_argument("--target-hwnd", type=_parse_handle, required=True)
    parser.add_argument("--interval-ms", type=int, default=100)
    parser.add_argument("--duration-ms", type=int, default=3_000)
    parser.add_argument(
        "--require-cleanup",
        action="store_true",
        help="fail unless observed indicator HWNDs disappear before the final sample",
    )
    parser.add_argument(
        "--motion-mode",
        choices=("auto", "animate", "reduce"),
        default="auto",
        help=(
            "auto is acceptance-authoritative; animate/reduce are diagnostic-only because "
            "Win32 does not expose the session override"
        ),
    )
    parser.add_argument("--theme", type=Path, default=default_theme)
    return parser


def main(
    argv: Sequence[str] | None = None,
    *,
    source: ObservationSource | None = None,
    stdout: TextIO = sys.stdout,
    sleep: Callable[[float], None] = time.sleep,
    timestamp: Callable[[], str] = _utc_timestamp,
    monotonic: Callable[[], float] = time.monotonic,
) -> int:
    args = _argument_parser().parse_args(argv)

    def emit(event: dict[str, Any]) -> None:
        event.setdefault("observation_only", True)
        print(
            json.dumps(
                event, ensure_ascii=False, separators=(",", ":"), sort_keys=True
            ),
            file=stdout,
            flush=True,
        )

    try:
        config = ProbeConfig(
            target_pid=args.target_pid,
            target_hwnd=args.target_hwnd,
            interval_ms=args.interval_ms,
            duration_ms=args.duration_ms,
            require_cleanup=args.require_cleanup,
            motion_mode=args.motion_mode,
        )
        contract = _load_contract(args.theme)
        observer = source if source is not None else Win32ObservationSource()
        return run_probe(
            observer,
            config,
            contract,
            emit,
            sleep=sleep,
            timestamp=timestamp,
            monotonic=monotonic,
        ).exit_code
    except ProbeFailure as error:
        reason = error.reason
    except (OSError, TypeError, ValueError) as error:
        reason = _reason(
            "probe_execution_failed",
            "the read-only acceptance probe could not complete",
            error=str(error),
        )
    emit(
        {
            "schema": SCHEMA,
            "event": "probe_failure",
            "observed_at": timestamp(),
            "reason": reason,
        }
    )
    emit(
        {
            "schema": SCHEMA,
            "event": "cleanup_summary",
            "observed_at": timestamp(),
            "status": "probe_failed",
            "cleanup_required": bool(getattr(args, "require_cleanup", False)),
            "convergence_observed": False,
            "ever_seen_count": 0,
            "removed_hwnds": [],
            "remaining_hwnds": [],
            "accepted": False,
        }
    )
    return 3


if __name__ == "__main__":
    raise SystemExit(main())
