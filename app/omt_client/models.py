"""Typed values exchanged at web-application boundaries."""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass, field
from typing import Any


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

    @classmethod
    def from_mapping(cls, value: Mapping[str, Any]) -> CommandResult:
        """Normalize the legacy diagnostic dictionary at one boundary."""
        sources = value.get("sources", ())
        return cls(
            command=str(value.get("command", "")),
            returncode=value.get("returncode"),
            stdout=str(value.get("stdout", "")),
            stderr=str(value.get("stderr", "")),
            duration_seconds=float(value.get("duration_seconds", 0.0) or 0.0),
            timed_out=bool(value.get("timed_out", False)),
            error=str(value.get("error", "")),
            skipped=bool(value.get("skipped", False)),
            stdout_truncated=bool(value.get("stdout_truncated", False)),
            stderr_truncated=bool(value.get("stderr_truncated", False)),
            sources=tuple(str(source) for source in sources or ()),
        )


@dataclass(frozen=True)
class DiagnosticResult:
    """A named diagnostic result suitable for the diagnostics page."""

    title: str
    command: CommandResult
    notes: tuple[str, ...] = field(default_factory=tuple)
