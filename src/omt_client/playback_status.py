"""The receiver's published playback-status contract.

This is the consumer half of a cross-language contract; the producer is
`crates/omt-receiver-core`. `tests/schema/
playback-status-vectors.json` holds the shared vectors and both suites assert
against it, so a field or state added on only one side fails the other.

It lives beside the other pure validation modules (`discovery.py`,
`network_config.py`, `records.py`) rather than inside `services/`: nothing here
touches the filesystem or a subprocess, and `services/playback.py` is only one
of its callers.
"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import UTC, datetime
from typing import TypeGuard

from .discovery import is_valid_direct_target, is_valid_source_name
from .json_document import JsonDocumentError, load_json_document

STATUS_FILE_LIMIT = 4096
STATUS_FUTURE_SKEW_SECONDS = 5
DETAIL_MAX_BYTES = 2048
DEVICE_MAX_BYTES = 256
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
CONNECTORS = frozenset({"none", "HDMI-A-1", "HDMI-A-2"})
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
# Must stay total over RECEIVER_STATES: PlaybackStatusRecord.projection indexes this
# directly, so a receiver state without a row would raise KeyError and 500 the
# dashboard. `tests/unit/test_playback_failures.py` asserts the totality.
PUBLIC_STATES: dict[str, tuple[str, str, str]] = {
    "running": ("playing", "Playing", "success"),
    "waiting-for-discovery": ("waiting-for-discovery", "Waiting for discovery", "warning"),
    "waiting-for-hdmi": ("waiting-for-hdmi", "Waiting for HDMI", "warning"),
    "retrying": ("retrying", "Retrying playback", "warning"),
    "degraded": ("degraded", "Playback degraded", "warning"),
    "unsupported-format": ("unsupported-format", "Unsupported video format", "danger"),
    "starting": ("starting", "Starting playback", "warning"),
    "stopped": ("stopped", "Playback stopped", "neutral"),
    "failed": ("failed", "Playback failed", "danger"),
}


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
            document = load_json_document(value)
        except JsonDocumentError as exc:
            raise ValueError("status is not valid JSON") from exc
        if (
            not isinstance(document, dict)
            or set(document) != STATUS_FIELDS
            or type(document.get("schema")) is not int
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
            or not _bounded_utf8(detail, DETAIL_MAX_BYTES)
            or not isinstance(connector, str)
            or connector not in CONNECTORS
            or not _bounded_utf8(drm_device, DEVICE_MAX_BYTES)
            or not drm_device
            or not _bounded_utf8(alsa_device, DEVICE_MAX_BYTES)
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

    def require_target(self, expected_target: str) -> None:
        """Reject a fresh record left behind by a different playback target."""
        if self.target != expected_target:
            raise ValueError("status target does not match the configured target")

    def require_fresh(self, now: datetime, maximum_age_seconds: float) -> None:
        """Raise `ValueError` unless this record was published recently enough.

        A future-dated record is rejected too: the receiver and the web worker
        share a clock, so a timestamp ahead of `now` by more than the tolerated
        skew means the record is not describing the present.
        """
        age = (now - self.updated_at).total_seconds()
        if age < -STATUS_FUTURE_SKEW_SECONDS or age > maximum_age_seconds:
            raise ValueError("status timestamp is stale or future-dated")

    def projection(self) -> tuple[str, str, str]:
        """Return the public `(state, label, tone)` shown on the dashboard."""
        return PUBLIC_STATES[self.state]
