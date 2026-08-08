"""Source discovery, target persistence, and playback control."""

from __future__ import annotations

import threading
import time
from datetime import UTC, datetime

from ..discovery import (
    OmtSourceChoice,
    is_valid_direct_target,
    parse_omt_sources,
    parse_source_selection,
)
from ..models import (
    ActionResult,
    CommandResult,
    PlaybackSummary,
    SourceConfigurationView,
    VideoLimitView,
)
from ..playback_status import STATUS_FILE_LIMIT, PlaybackStatusRecord
from ..safe_io import read_bytes
from ..settings import AppSettings
from ..state_store import (
    SourceConfigurationError,
    SourceTarget,
    VideoCeilingError,
    describe_video_ceiling,
    effective_video_ceiling,
    parse_video_ceiling,
    read_source_target,
    save_source_target,
    save_video_ceiling,
)
from .command import run_command


class RuntimeSourcePlayback:
    def __init__(self, settings: AppSettings) -> None:
        self._settings = settings
        self._cache: tuple[float, list[OmtSourceChoice]] = (0.0, [])
        self._cache_condition = threading.Condition()
        self._discovery_in_progress = False
        self._cache_generation = 0
        # Bumped by `refresh`. A discovery carries the epoch it started under so
        # its answer cannot be published against a later invalidation.
        self._refresh_epoch = 0

    def _target(self) -> SourceTarget | None:
        return read_source_target(self._settings.source_target_file)

    def sources(self) -> list[OmtSourceChoice]:
        with self._cache_condition:
            if time.monotonic() < self._cache[0]:
                return list(self._cache[1])
            observed_generation = self._cache_generation
            while self._discovery_in_progress:
                self._cache_condition.wait()
                # A caller that actually waited for this discovery shares its
                # answer even when caching is disabled with a zero TTL. Without
                # a generation check, every awakened waiter sees the new entry
                # as already expired and launches the same receiver command
                # again in sequence.
                if self._cache_generation != observed_generation:
                    return list(self._cache[1])
            self._discovery_in_progress = True
            epoch = self._refresh_epoch
        try:
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
        except BaseException:
            # Never strand callers behind an in-flight marker if an unexpected
            # dependency failure escapes the bounded command adapter.
            with self._cache_condition:
                self._discovery_in_progress = False
                self._cache_condition.notify_all()
            raise
        # Anchor the expiry to the moment the answer became known. Discovery
        # blocks for at least --wait-ms, so anchoring to the pre-command clock
        # would spend that time out of the TTL -- and with a TTL shorter than
        # the discovery itself the entry would be born expired, making every
        # dashboard render pay for another multi-second discovery.
        with self._cache_condition:
            # A refresh that arrived while this discovery was running invalidated
            # exactly the network state it was observing, so publishing the answer
            # now would reinstate the list the refresh was asked to discard --
            # and, because waiters key off the generation, hand it to them as
            # though it were the fresh one they waited for. Leave the cache
            # expired instead; the next caller discovers again.
            if epoch == self._refresh_epoch:
                self._cache = (time.monotonic() + self._settings.source_cache_ttl_seconds, choices)
                self._cache_generation += 1
            self._discovery_in_progress = False
            self._cache_condition.notify_all()
        return list(choices)

    def configuration(self) -> SourceConfigurationView:
        try:
            target = self._target()
        except SourceConfigurationError as exc:
            return SourceConfigurationView(error=str(exc))
        if target is None:
            return SourceConfigurationView()
        if target.kind == "discovered":
            return SourceConfigurationView(source=target.value)
        return SourceConfigurationView(source=target.value, direct_address=target.value)

    def video_limit(self) -> VideoLimitView:
        """The decode ceiling in force, and the board default behind it."""
        board_default = self._settings.board_video_ceiling
        try:
            effective = effective_video_ceiling(self._settings.video_ceiling_file, board_default)
        except VideoCeilingError as exc:
            return VideoLimitView(
                board_label=self._settings.board_label,
                effective=board_default,
                board_default=board_default,
                error=str(exc),
            )
        return VideoLimitView(
            board_label=self._settings.board_label,
            effective=effective,
            board_default=board_default,
        )

    def save_video_limit(self, ceiling: str) -> ActionResult:
        """Set or clear the operator's ceiling override and restart playback.

        An empty value clears the override rather than saving one, so returning
        to the board default is the same control the operator used to leave it.
        The limit is not clamped to the board default: raising it is the
        operator's call, and the system page says what that costs.
        """
        requested = ceiling.strip()
        try:
            normalized = None if not requested else parse_video_ceiling(requested)
            save_video_ceiling(self._settings.video_ceiling_file, normalized)
        except VideoCeilingError as exc:
            return ActionResult(False, error=str(exc))
        label = (
            "Video limit cleared"
            if normalized is None
            else f"Video limit set to {describe_video_ceiling(normalized)}"
        )
        restarted = self._control("restart")
        if restarted.returncode == 0:
            return ActionResult(True, message=f"{label} and playback restarted.")
        return ActionResult(
            False,
            error=(f"{label}, but playback could not be restarted. {restarted.failure_detail}"),
        )

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
        return ActionResult(
            False,
            error=(
                f"{label} was saved, but playback could not be restarted. "
                f"{restarted.failure_detail}"
            ),
        )

    def select(self, selection: str) -> ActionResult:
        parsed = parse_source_selection(selection.strip())
        if not parsed:
            return ActionResult(False, error="Invalid OMT source selection.")
        source, address, backend = parsed
        target = SourceTarget("direct" if address else "discovered", address or source)
        return self._save_and_restart(target, backend)

    def refresh(self) -> None:
        with self._cache_condition:
            self._cache = (0.0, [])
            self._refresh_epoch += 1

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
        return ActionResult(
            False,
            error=f"Unable to restart OMT playback. {result.failure_detail}",
        )

    def clear(self) -> ActionResult:
        stopped = self._control("stop")
        if stopped.returncode not in (0, 3):
            return ActionResult(
                False,
                error=(
                    "Playback could not be stopped, so the saved target was retained. "
                    + stopped.failure_detail
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
        try:
            target = self._target()
        except SourceConfigurationError as exc:
            return PlaybackSummary(
                "configuration-error",
                "Source configuration invalid",
                str(exc),
                "danger",
            )
        if target is None:
            return PlaybackSummary(
                "unconfigured",
                "No source configured",
                "Select a discovered source or configure a direct OMT target.",
                "neutral",
            )
        source = target.value
        address = source if target.kind == "direct" else ""
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
            status.require_target(source)
            status.require_fresh(
                datetime.now(UTC),
                self._settings.playback_status_stale_seconds,
            )
        except ValueError:
            return PlaybackSummary(
                "stale",
                "Playback status stale",
                "The receiver status record is unavailable or stale.",
                "warning",
                source,
                address,
            )
        public_state, label, tone = status.projection()
        return PlaybackSummary(
            public_state,
            label,
            status.detail,
            tone,
            source,
            address,
        )
