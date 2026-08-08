"""Typed values exchanged at web-application boundaries."""

from __future__ import annotations

from dataclasses import dataclass

from .state_store import describe_video_ceiling


@dataclass(frozen=True)
class SourceConfigurationView:
    """Configured OMT target as routes and diagnostics should present it."""

    source: str = ""
    direct_address: str = ""
    error: str = ""

    @property
    def configured(self) -> bool:
        return bool(self.source) and not self.error


@dataclass(frozen=True)
class VideoLimitView:
    """The decode ceiling as the system page should present it."""

    board_label: str
    effective: str
    board_default: str
    error: str = ""

    @property
    def effective_description(self) -> str:
        return describe_video_ceiling(self.effective)

    @property
    def board_default_description(self) -> str:
        return describe_video_ceiling(self.board_default)

    @property
    def overridden(self) -> bool:
        return not self.error and self.effective != self.board_default

    @property
    def above_board_default(self) -> bool:
        """Whether the operator raised the limit past what the board is rated for.

        Allowed, but worth saying: an over-rated board drops frames rather than
        reporting `unsupported-format`, which looks like a network fault.
        """
        return self.overridden and _pixel_rate(self.effective) > _pixel_rate(self.board_default)


def _pixel_rate(ceiling: str) -> int:
    """Peak pixels per second a ceiling permits, for comparison only."""
    peak = 0
    for shape in ceiling.split(","):
        dimensions, _, rate = shape.partition("@")
        width, _, height = dimensions.partition("x")
        try:
            peak = max(peak, int(width) * int(height) * int(rate))
        except ValueError:
            return 0
    return peak


@dataclass(frozen=True)
class ActionResult:
    """Outcome of an operator-requested state change."""

    ok: bool
    message: str = ""
    error: str = ""


@dataclass(frozen=True)
class PlaybackSummary:
    """Small, presentation-neutral summary of current playback health."""

    state: str
    label: str
    detail: str
    tone: str
    source: str = ""
    direct_address: str = ""


@dataclass(frozen=True)
class CommandResult:
    """Bounded subprocess result exposed to an authenticated operator."""

    command: str = ""
    returncode: int | None = None
    stdout: str = ""
    stderr: str = ""
    duration_seconds: float = 0.0
    timed_out: bool = False
    error: str = ""
    skipped: bool = False
    stdout_truncated: bool = False
    stderr_truncated: bool = False
    sources: tuple[str, ...] = ()

    @property
    def failure_detail(self) -> str:
        """Return the most specific explanation of a failure, or "".

        `error` is this process's own account (timeout, spawn failure), so it
        outranks anything the command printed; a failing command explains itself
        on stderr before stdout. Callers append this to their own sentence, so an
        entirely silent failure contributes nothing rather than "unavailable".
        """
        return self.error or self.stderr.strip() or self.stdout.strip()

    @property
    def report_text(self) -> str:
        """Return what a command *said*, for display rather than for a failure.

        Inverse precedence to `failure_detail`: a command that both printed a
        result and failed is being shown for its output here, so stdout wins.
        """
        return self.stdout.strip() or self.error or self.stderr.strip() or "unavailable"


@dataclass(frozen=True)
class DiagnosticResult:
    """A named diagnostic result suitable for the diagnostics page."""

    title: str
    command: CommandResult
