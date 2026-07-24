"""Shared request helpers for route modules."""

from __future__ import annotations

from collections.abc import Callable
from functools import wraps
from typing import Any, TypeVar, cast

from flask import current_app, redirect, session, url_for

from ..services import ServiceContainer

F = TypeVar("F", bound=Callable[..., Any])


def services() -> ServiceContainer:
    return cast(ServiceContainer, current_app.extensions["omt_client.services"])


def authenticated() -> bool:
    if not session.get("authenticated"):
        return False
    try:
        valid = services().auth.is_current()
    except Exception:
        current_app.logger.exception("Unable to validate the persistent web session")
        valid = False
    if not valid:
        session.clear()
    return valid


def login_required(function: F) -> F:
    @wraps(function)
    def decorated(*args: Any, **kwargs: Any) -> Any:
        if not authenticated():
            return redirect(url_for("auth.login"))
        return function(*args, **kwargs)

    return decorated  # type: ignore[return-value]


def publish_action(result: Any) -> None:
    from flask import flash

    if result.ok:
        flash(result.message, "success")
    else:
        flash(result.error, "error")
    for detail in result.details:
        flash(detail, "warning" if not result.ok else "success")
