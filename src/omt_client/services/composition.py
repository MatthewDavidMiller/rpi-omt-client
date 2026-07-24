"""Production service composition root."""

from __future__ import annotations

import re

from ..settings import AppSettings, load_settings
from .authentication import PersistentAuthentication
from .diagnostics import RuntimeDiagnostics
from .host_system import HostSystem
from .network import RuntimeNetwork
from .playback import RuntimeSourcePlayback
from .protocols import ServiceContainer


def production_services(settings: AppSettings | None = None) -> ServiceContainer:
    effective = settings or load_settings()
    source = RuntimeSourcePlayback(effective)
    return ServiceContainer(
        auth=PersistentAuthentication(effective),
        source=source,
        network=RuntimeNetwork(effective, source),
        diagnostics=RuntimeDiagnostics(effective, source),
        system=HostSystem(effective),
    )


def controller_pid(status: str) -> int | None:
    match = re.match(r"^running:([1-9][0-9]*)(?:\s|$)", status.strip())
    return int(match.group(1)) if match else None
