"""HTTP route blueprints for the OMT Client web UI."""

from .about import about_blueprint
from .auth import auth_blueprint
from .dashboard import dashboard_blueprint
from .diagnostics import diagnostics_blueprint
from .network import network_blueprint
from .system import system_blueprint

__all__ = (
    "about_blueprint",
    "auth_blueprint",
    "dashboard_blueprint",
    "diagnostics_blueprint",
    "network_blueprint",
    "system_blueprint",
)
