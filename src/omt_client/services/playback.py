"""Source discovery, target persistence, and typed playback status."""

from __future__ import annotations

import json
import threading
import time
from dataclasses import dataclass
from datetime import UTC, datetime
from typing import TypeGuard

from ..discovery import (
    OmtSourceChoice,
    is_valid_direct_target,
    is_valid_source_name,
    parse_omt_sources,
    parse_source_selection,
)
from ..models import ActionResult, CommandResult, PlaybackSummary
from ..safe_io import read_bytes
from ..settings import AppSettings
from ..state_store import (
    SourceConfigurationError,
    SourceTarget,
    read_source_target,
    save_source_target,
)
from .command import run_command

STATUS_FILE_LIMIT = 4096
STATUS_FUTURE_SKEW_SECONDS = 5
RECEIVER_STATES = frozenset(
    {
        "running",
        "waiting-for-discovery",
        "waiting-for-hdmi",
        "retrying",
        "degraded",
        "unsupported-format",
        "starting",
        "stopped",
        "failed",
    }
)
VIDEO_STATES = RECEIVER_STATES - {"degraded"}
AUDIO_STATES = frozenset({"stopped", "running", "failed"})
STATUS_FIELDS = frozenset(
    {
        "schema",
        "state",
        "video_state",
        "audio_state",
        "target",
        "detail",
        "connector",
        "drm_device",
        "alsa_device",
        "updated_at",
    }
)


def _bounded_utf8(value: object, maximum_bytes: int) -> TypeGuard[str]:
    if not isinstance(value, str):
        return False
    try:
        return len(value.encode("utf-8")) <= maximum_bytes
    except UnicodeEncodeError:
        return False


@dataclass(frozen=True)
class PlaybackStatusRecord:
    state: str
    video_state: str
    audio_state: str
    target: str
    detail: str
    updated_at: datetime

    @classmethod
    def parse(cls, value: bytes) -> PlaybackStatusRecord:
        try:
            document = json.loads(value)
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            raise ValueError("status is not valid JSON") from exc
        if (
            not isinstance(document, dict)
            or set(document) != STATUS_FIELDS
            or document.get("schema") != 1
        ):
            raise ValueError("status has an invalid schema")
        state = document.get("state")
        video_state = document.get("video_state")
        audio_state = document.get("audio_state")
        target = document.get("target")
        detail = document.get("detail")
        connector = document.get("connector")
        drm_device = document.get("drm_device")
        alsa_device = document.get("alsa_device")
        updated_raw = document.get("updated_at")
        if (
            not isinstance(state, str)
            or state not in RECEIVER_STATES
            or not isinstance(video_state, str)
            or video_state not in VIDEO_STATES
            or not isinstance(audio_state, str)
            or audio_state not in AUDIO_STATES
            or not isinstance(target, str)
            or not (is_valid_source_name(target) or is_valid_direct_target(target))
            or not _bounded_utf8(detail, 2048)
            or not isinstance(connector, str)
            or connector not in {"none", "HDMI-A-1", "HDMI-A-2"}
            or not _bounded_utf8(drm_device, 256)
            or not drm_device
            or not _bounded_utf8(alsa_device, 256)
            or not alsa_device
            or not isinstance(updated_raw, str)
        ):
            raise ValueError("status fields are invalid")
        if (
            (state == "degraded" and (video_state != "running" or audio_state != "failed"))
            or (state == "running" and (video_state != "running" or audio_state == "failed"))
            or (state not in {"running", "degraded"} and state != video_state)
        ):
            raise ValueError("status state projection is invalid")
        try:
            updated = datetime.fromisoformat(updated_raw.replace("Z", "+00:00"))
        except ValueError as exc:
            raise ValueError("status timestamp is invalid") from exc
        if updated.tzinfo is None:
            raise ValueError("status timestamp lacks a timezone")
        return cls(
            state,
            video_state,
            audio_state,
            target,
            detail,
            updated.astimezone(UTC),
        )


class RuntimeSourcePlayback:
    def __init__(self, settings: AppSettings) -> None:
        self._settings = settings
        self._cache: tuple[float, list[OmtSourceChoice]] = (0.0, [])
        self._cache_lock = threading.Lock()

    def _target(self) -> SourceTarget | None:
        return read_source_target(self._settings.source_target_file)

    def sources(self) -> list[OmtSourceChoice]:
        now = time.monotonic()
        with self._cache_lock:
            if now < self._cache[0]:
                return list(self._cache[1])
        result = run_command(
            [
                self._settings.receiver_command,
                "discover",
                "--wait-ms",
                "1500",
                "--json",
            ],
            max(3.0, self._settings.control_timeout_seconds),
        )
        choices = (
            [OmtSourceChoice(name) for name in parse_omt_sources(result.stdout)]
            if result.returncode == 0
            else []
        )
        with self._cache_lock:
            self._cache = (now + self._settings.source_cache_ttl_seconds, choices)
        return list(choices)

    def configuration(self) -> tuple[str, str]:
        try:
            target = self._target()
        except SourceConfigurationError:
            return "", ""
        if target is None:
            return "", ""
        return (target.value, "") if target.kind == "discovered" else (target.value, target.value)

    def _control(self, action: str) -> CommandResult:
        return run_command(
            [self._settings.control_command, action],
            self._settings.control_timeout_seconds,
        )

    def _save_and_restart(self, target: SourceTarget, label: str) -> ActionResult:
        try:
            save_source_target(self._settings.source_target_file, target)
        except SourceConfigurationError as exc:
            return ActionResult(False, error=str(exc))
        self.refresh()
        restarted = self._control("restart")
        if restarted.returncode == 0:
            return ActionResult(True, message=f"{label} saved and running.")
        detail = restarted.error or restarted.stderr.strip() or restarted.stdout.strip()
        return ActionResult(
            False,
            error=f"{label} was saved, but playback could not be restarted. {detail}",
        )

    def select(self, selection: str) -> ActionResult:
        parsed = parse_source_selection(selection.strip())
        if not parsed:
            return ActionResult(False, error="Invalid OMT source selection.")
        source, address, backend = parsed
        target = SourceTarget("direct" if address else "discovered", address or source)
        return self._save_and_restart(target, backend)

    def refresh(self) -> None:
        with self._cache_lock:
            self._cache = (0.0, [])

    def restart(self) -> ActionResult:
        try:
            target = self._target()
        except SourceConfigurationError as exc:
            return ActionResult(False, error=f"Saved OMT target is invalid: {exc}")
        if target is None:
            return ActionResult(False, error="No OMT source is configured.")
        result = self._control("restart")
        if result.returncode == 0:
            return ActionResult(True, message="OMT playback restarted.")
        detail = result.error or result.stderr.strip() or result.stdout.strip()
        return ActionResult(False, error=f"Unable to restart OMT playback. {detail}")

    def clear(self) -> ActionResult:
        stopped = self._control("stop")
        if stopped.returncode not in (0, 3):
            detail = stopped.error or stopped.stderr.strip() or stopped.stdout.strip()
            return ActionResult(
                False,
                error=(
                    "Playback could not be stopped, so the saved target was retained. " + detail
                ),
            )
        try:
            save_source_target(self._settings.source_target_file, None)
        except SourceConfigurationError as exc:
            return ActionResult(
                False,
                error=f"Playback stopped, but the saved target could not be cleared. {exc}",
            )
        self.refresh()
        return ActionResult(True, message="Playback stopped and the saved target was cleared.")

    def save_direct(self, address: str) -> ActionResult:
        if not is_valid_direct_target(address):
            return ActionResult(
                False,
                error="Direct target must use omt://host:port with no path or credentials.",
            )
        return self._save_and_restart(SourceTarget("direct", address), "OMT direct target")

    def playback(self) -> PlaybackSummary:
        source, address = self.configuration()
        if not source:
            return PlaybackSummary(
                "unconfigured",
                "No source configured",
                "Select a discovered source or configure a direct OMT target.",
                "neutral",
            )
        result = read_bytes(self._settings.playback_status_file, STATUS_FILE_LIMIT)
        if not result.ok:
            control = self._control("status")
            if control.returncode == 0:
                return PlaybackSummary(
                    "starting",
                    "Starting playback",
                    "The receiver is running and has not published fresh status yet.",
                    "warning",
                    source,
                    address,
                )
            return PlaybackSummary(
                "stopped",
                "Playback stopped",
                "A target is saved but the receiver is not running.",
                "neutral",
                source,
                address,
            )
        try:
            status = PlaybackStatusRecord.parse(result.data)
            age = (datetime.now(UTC) - status.updated_at).total_seconds()
            if (
                age < -STATUS_FUTURE_SKEW_SECONDS
                or age > self._settings.playback_status_stale_seconds
            ):
                raise ValueError("status timestamp is stale or future-dated")
        except ValueError:
            return PlaybackSummary(
                "stale",
                "Playback status stale",
                "The receiver status record is unavailable or stale.",
                "warning",
                source,
                address,
            )
        mapping = {
            "running": ("playing", "Playing", "success"),
            "waiting-for-discovery": (
                "waiting-for-discovery",
                "Waiting for discovery",
                "warning",
            ),
            "waiting-for-hdmi": ("waiting-for-hdmi", "Waiting for HDMI", "warning"),
            "retrying": ("retrying", "Retrying playback", "warning"),
            "degraded": ("degraded", "Playback degraded", "warning"),
            "unsupported-format": (
                "unsupported-format",
                "Unsupported video format",
                "danger",
            ),
            "starting": ("starting", "Starting playback", "warning"),
            "stopped": ("stopped", "Playback stopped", "neutral"),
            "failed": ("failed", "Playback failed", "danger"),
        }
        public_state, label, tone = mapping[status.state]
        return PlaybackSummary(
            public_state,
            label,
            status.detail,
            tone,
            source,
            address,
        )
