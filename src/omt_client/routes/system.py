"""High-impact host-system actions."""

from flask import Blueprint, redirect, render_template, url_for

from .common import login_required, publish_action, services

system_blueprint = Blueprint("system", __name__)


@system_blueprint.get("/system")
@login_required
def system():
    return render_template("system.html")


@system_blueprint.get("/system/reboot")
@login_required
def confirm_reboot():
    return render_template("reboot_confirm.html")


@system_blueprint.post("/system/reboot")
@login_required
def reboot():
    result = services().system.request_reboot()
    if result.ok:
        return render_template("reboot_scheduled.html", message=result.message), 202
    publish_action(result)
    return redirect(url_for("system.system"))
