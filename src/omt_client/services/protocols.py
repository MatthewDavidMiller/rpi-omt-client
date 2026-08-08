"""Typed service interfaces exposed to HTTP routes."""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass
from typing import IO, Any, Protocol

from ..models import (
    ActionResult,
    DiagnosticResult,
    PlaybackSummary,
    SourceConfigurationView,
    VideoLimitView,
)


class SourceChoice(Protocol):
    """Everything `dashboard.html` renders for one selectable source.

    The template consumes this contract directly rather than testing each
    attribute for existence, so any service that supplies sources owes the
    dashboard all of it -- including `backend`, which the discovered-source
    list shows as a badge.
    """

    @property
    def name(self) -> str: ...

    @property
    def address(self) -> str: ...

    @property
    def backend(self) -> str: ...

    @property
    def selection_value(self) -> str: ...

    @property
    def display_label(self) -> str: ...


class AuthenticationService(Protocol):
    @property
    def secret_key(self) -> str | bytes: ...

    @property
    def password_digest(self) -> str:
        """Bind a browser session to the password it was issued against."""
        ...

    def authenticate(self, password: str, previous_session_id: str | None) -> str | None: ...
    def is_current(self) -> bool: ...
    def revoke(self, session_id: str | None) -> None: ...


class AboutService(Protocol):
    def version(self) -> str: ...
    def legal_texts(self) -> tuple[str, str]: ...


class SourcePlaybackService(Protocol):
    def sources(self) -> Sequence[SourceChoice]: ...
    def configuration(self) -> SourceConfigurationView: ...
    def playback(self) -> PlaybackSummary: ...
    def select(self, selection: str) -> ActionResult: ...
    def refresh(self) -> None: ...
    def restart(self) -> ActionResult: ...
    def clear(self) -> ActionResult: ...
    def save_direct(self, address: str) -> ActionResult: ...
    def video_limit(self) -> VideoLimitView: ...
    def save_video_limit(self, ceiling: str) -> ActionResult: ...


class NetworkService(Protocol):
    def read(self) -> dict[str, Any]: ...
    def save(self, discovery_server: str) -> ActionResult: ...


class DiagnosticsService(Protocol):
    def status(self) -> str: ...
    def discovery(self) -> DiagnosticResult: ...
    def runtime(self) -> tuple[DiagnosticResult, str]:
        """Return the runtime check and the controller status it observed.

        Both come from one `control-omt.sh status`, so the page cannot show a
        header that disagrees with the check printed underneath it.
        """
        ...

    def direct(self, address: str) -> DiagnosticResult: ...
    def bundle(self, include_packet_capture: bool = False) -> tuple[IO[bytes], str]: ...


class SystemService(Protocol):
    def request_reboot(self) -> ActionResult: ...


@dataclass(frozen=True)
class ServiceContainer:
    auth: AuthenticationService
    about: AboutService
    source: SourcePlaybackService
    network: NetworkService
    diagnostics: DiagnosticsService
    system: SystemService
