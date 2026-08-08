"""High-impact host-system actions."""

from __future__ import annotations

from flask import Blueprint, redirect, render_template, request, url_for
from flask.typing import ResponseReturnValue

from .common import login_required, publish_action, services

system_blueprint = Blueprint("system", __name__)


@system_blueprint.get("/system")
@login_required
def system() -> ResponseReturnValue:
    return render_template("system.html", video_limit=services().source.video_limit())


@system_blueprint.post("/system/video-limit")
@login_required
def save_video_limit() -> ResponseReturnValue:
    result = services().source.save_video_limit(request.form.get("video_limit", ""))
    publish_action(result)
    return redirect(url_for("system.system"))


@system_blueprint.get("/system/reboot")
@login_required
def confirm_reboot() -> ResponseReturnValue:
    return render_template("reboot_confirm.html")


@system_blueprint.post("/system/reboot")
@login_required
def reboot() -> ResponseReturnValue:
    result = services().system.request_reboot()
    if result.ok:
        return render_template("reboot_scheduled.html", message=result.message), 202
    publish_action(result)
    return redirect(url_for("system.system"))
