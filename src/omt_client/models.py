"""Typed values exchanged at web-application boundaries."""

from __future__ import annotations

from dataclasses import dataclass


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
