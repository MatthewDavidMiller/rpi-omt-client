"""Side-effect-free services for the local production-template preview.

This package is deliberately a sibling of ``omt_client`` rather than a module
inside it: ``deploy/Dockerfile`` copies only ``src/omt_client/``, so keeping the
fakes here guarantees they never reach the appliance image or its
``runtime-sha256.manifest``.
"""

from __future__ import annotations

import hashlib
import hmac
import io
import secrets
import zipfile
from dataclasses import dataclass
from typing import Any

from flask import session

from omt_client.models import ActionResult, CommandResult, DiagnosticResult, PlaybackSummary
from omt_client.services import ServiceContainer


@dataclass(frozen=True)
class PreviewSourceChoice:
    name: str
    backend: str = "OMT discovery"
    address: str = ""

    @property
    def selection_value(self) -> str:
        return f"discovered|{self.name}"

    @property
    def display_label(self) -> str:
        suffix = f" · {self.backend}"
        return f"{self.name}{suffix}"


class PreviewAuthentication:
    secret_key = "preview-dev-secret"

    def __init__(self, password: str) -> None:
        self._password = password
        self._sessions: set[str] = set()

    @property
    def password_digest(self) -> str:
        """Mirror the production session-to-password binding."""
        return hmac.new(
            self.secret_key.encode("utf-8"), self._password.encode("utf-8"), hashlib.sha256
        ).hexdigest()

    def authenticate(self, password: str, previous_session_id: str | None) -> str | None:
        if not hmac.compare_digest(password.encode(), self._password.encode()):
            return None
        if previous_session_id:
            self._sessions.discard(previous_session_id)
        session_id = secrets.token_urlsafe(24)
        self._sessions.add(session_id)
        return session_id

    def is_current(self) -> bool:
        return session.get("session_id") in self._sessions

    def revoke(self, session_id: str | None) -> None:
        if session_id:
            self._sessions.discard(session_id)


class PreviewSourcePlayback:
    def __init__(self) -> None:
        self._source = "STUDIO-PC (OBS Studio)"
        self._address = ""
        self._playing = True
        self._sources = [
            PreviewSourceChoice("STUDIO-PC (OBS Studio)"),
            PreviewSourceChoice("MEDIA-SERVER (Channel 1)"),
            PreviewSourceChoice("PRODUCER-MAC (OMT Virtual Input)"),
            PreviewSourceChoice("GRAPHICS-PC (CasparCG)"),
            PreviewSourceChoice("REMOTE-CAMERA"),
        ]

    def sources(self) -> list[PreviewSourceChoice]:
        return list(self._sources)

    def configuration(self) -> tuple[str, str]:
        return self._source, self._address

    def playback(self) -> PlaybackSummary:
        if not self._source:
            return PlaybackSummary(
                "unconfigured",
                "No source configured",
                "Select a source to begin playback.",
                "neutral",
            )
        if self._playing:
            return PlaybackSummary(
                "playing",
                "Playing",
                "Video and audio pipelines are running.",
                "success",
                self._source,
                self._address,
            )
        return PlaybackSummary(
            "stopped",
            "Playback stopped",
            "A source is saved but playback is stopped.",
            "neutral",
            self._source,
            self._address,
        )

    def select(self, selection: str) -> ActionResult:
        name = selection.removeprefix("discovered|")
        choice = next((item for item in self._sources if item.name == name), None)
        if not choice:
            return ActionResult(False, error="Invalid OMT source name.")
        self._source, self._address, self._playing = choice.name, choice.address, True
        return ActionResult(True, message="Preview source saved and running.")

    def refresh(self) -> None:
        return None

    def restart(self) -> ActionResult:
        if not self._source:
            return ActionResult(False, error="No source is configured.")
        self._playing = True
        return ActionResult(True, message="Preview playback restarted.")

    def clear(self) -> ActionResult:
        self._playing = False
        self._source = ""
        self._address = ""
        return ActionResult(True, message="Preview playback stopped and source cleared.")

    def save_direct(self, address: str) -> ActionResult:
        if not address.startswith("omt://") or ":" not in address[6:]:
            return ActionResult(False, error="Invalid direct-connect source or address.")
        self._source, self._address, self._playing = address, address, True
        return ActionResult(True, message="Preview direct source saved and running.")


class PreviewNetwork:
    def __init__(self) -> None:
        self._value: dict[str, Any] = {
            "discovery_server": "",
            "discovery_server_text": "",
            "error": "",
        }

    def read(self) -> dict[str, Any]:
        return dict(self._value)

    def save(self, discovery_server: str) -> ActionResult:
        self._value.update(
            discovery_server=discovery_server,
            discovery_server_text=discovery_server,
        )
        return ActionResult(True, message="Preview network settings saved.")


class PreviewDiagnostics:
    def version(self) -> str:
        return "preview"

    def status(self) -> str:
        return "running:4242 overall=running video=running audio=running"

    def discovery(self) -> DiagnosticResult:
        return DiagnosticResult(
            "Discovery check",
            CommandResult(
                command="omt-receiver discover --wait-ms 3000 --json",
                returncode=0,
                stdout='[{"name":"STUDIO-PC (OBS Studio)","target":"STUDIO-PC (OBS Studio)"}]\n',
                duration_seconds=0.1,
                sources=("STUDIO-PC (OBS Studio)",),
            ),
        )

    def runtime(self) -> DiagnosticResult:
        return DiagnosticResult(
            "Runtime check",
            CommandResult(
                command="/bin/sh -c runtime-check",
                returncode=0,
                stdout=(
                    "## identity\nuid=1000(preview) gid=1000(preview)\n\n"
                    "## devices\n/dev/dri/card1\n/dev/snd/controlC0\n"
                ),
                duration_seconds=0.1,
            ),
        )

    def direct(self, address: str) -> DiagnosticResult:
        return DiagnosticResult(
            "Direct-connect check",
            CommandResult(
                command=f"omt-receiver probe --target {address} --timeout-ms 3000 --json",
                returncode=0,
                duration_seconds=0.1,
            ),
        )

    def bundle(self, include_packet_capture: bool = False) -> tuple[io.BytesIO, str]:
        bundle = io.BytesIO()
        with zipfile.ZipFile(bundle, "w") as archive:
            archive.writestr("preview.txt", "Preview bundle: no host data was collected.\n")
            archive.writestr(
                "host-network-pcap.txt",
                f"capture_status=preview\ncapture_requested={int(include_packet_capture)}\n",
            )
        bundle.seek(0)
        return bundle, "omt-diagnostics-preview.zip"


class PreviewSystem:
    def request_reboot(self) -> ActionResult:
        return ActionResult(
            True,
            message="Preview reboot scheduled. No host action was performed.",
        )


def preview_services(password: str = "omt-client") -> ServiceContainer:
    source = PreviewSourcePlayback()
    return ServiceContainer(
        auth=PreviewAuthentication(password),
        source=source,
        network=PreviewNetwork(),
        diagnostics=PreviewDiagnostics(),
        system=PreviewSystem(),
    )
