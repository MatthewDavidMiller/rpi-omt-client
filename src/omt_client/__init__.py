"""Raspberry Pi OMT Client web application package."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from .factory import create_app as create_app
    from .models import ActionResult, CommandResult, DiagnosticResult, PlaybackSummary
    from .services import ServiceContainer

__all__ = (
    "ActionResult",
    "CommandResult",
    "DiagnosticResult",
    "PlaybackSummary",
    "ServiceContainer",
    "create_app",
)


def __getattr__(name: str) -> Any:
    """Lazy exports so `python -m omt_client.state_store` needs no Flask stack."""
    if name == "create_app":
        from .factory import create_app

        return create_app
    if name in {"ActionResult", "CommandResult", "DiagnosticResult", "PlaybackSummary"}:
        from . import models

        return getattr(models, name)
    if name == "ServiceContainer":
        from .services import ServiceContainer

        return ServiceContainer
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
