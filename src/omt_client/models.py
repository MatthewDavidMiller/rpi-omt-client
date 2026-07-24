"""Typed values exchanged at web-application boundaries."""

from __future__ import annotations

from dataclasses import dataclass, field


@dataclass(frozen=True)
class ActionResult:
    """Outcome of an operator-requested state change."""

    ok: bool
    message: str = ""
    error: str = ""
    details: tuple[str, ...] = ()


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


@dataclass(frozen=True)
class DiagnosticResult:
    """A named diagnostic result suitable for the diagnostics page."""

    title: str
    command: CommandResult
    notes: tuple[str, ...] = field(default_factory=tuple)
