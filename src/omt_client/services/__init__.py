"""Typed service contracts and runtime implementations."""

from .authentication import PersistentAuthentication
from .composition import production_services
from .diagnostics import RuntimeDiagnostics
from .host_system import HostSystem
from .network import RuntimeNetwork
from .playback import RuntimeSourcePlayback
from .protocols import (
    AuthenticationService,
    DiagnosticsService,
    NetworkService,
    ServiceContainer,
    SourcePlaybackService,
    SystemService,
)

__all__ = (
    "AuthenticationService",
    "DiagnosticsService",
    "HostSystem",
    "NetworkService",
    "PersistentAuthentication",
    "RuntimeDiagnostics",
    "RuntimeNetwork",
    "RuntimeSourcePlayback",
    "ServiceContainer",
    "SourcePlaybackService",
    "SystemService",
    "production_services",
)
