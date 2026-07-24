"""Shared request helpers for route modules."""

from __future__ import annotations

from collections.abc import Callable
from functools import wraps
from typing import ParamSpec, cast

from flask import current_app, flash, redirect, session, url_for
from flask.typing import ResponseReturnValue

from ..models import ActionResult
from ..services import ServiceContainer

P = ParamSpec("P")


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


def login_required(
    function: Callable[P, ResponseReturnValue],
) -> Callable[P, ResponseReturnValue]:
    @wraps(function)
    def decorated(*args: P.args, **kwargs: P.kwargs) -> ResponseReturnValue:
        if not authenticated():
            return redirect(url_for("auth.login"))
        return function(*args, **kwargs)

    return decorated


def publish_action(result: ActionResult) -> None:
    if result.ok:
        flash(result.message, "success")
    else:
        flash(result.error, "error")
