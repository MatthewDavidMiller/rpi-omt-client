"""Production service composition root."""

from __future__ import annotations

from ..settings import AppSettings, load_settings
from .about import RuntimeAbout
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
        about=RuntimeAbout(effective),
        source=source,
        network=RuntimeNetwork(effective, source),
        diagnostics=RuntimeDiagnostics(effective, source),
        system=HostSystem(effective),
    )
