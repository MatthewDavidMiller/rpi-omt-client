"""Dashboard and playback-control routes."""

from flask import Blueprint, redirect, render_template, request, url_for

from .common import login_required, publish_action, services

dashboard_blueprint = Blueprint("dashboard", __name__)


@dashboard_blueprint.get("/")
@login_required
def dashboard():
    source_choices = services().source.sources()
    current_source, current_address = services().source.configuration()
    return render_template(
        "dashboard.html",
        sources=source_choices,
        current_source=current_source,
        current_source_url_address=current_address,
        playback=services().source.playback(),
    )


@dashboard_blueprint.post("/sources/select")
@login_required
def select_source():
    publish_action(services().source.select(request.form.get("source", "")))
    return redirect(url_for("dashboard.dashboard"))


@dashboard_blueprint.post("/sources/refresh")
@login_required
def refresh_sources():
    services().source.refresh()
    return redirect(url_for("dashboard.dashboard"))


@dashboard_blueprint.post("/playback/restart")
@login_required
def restart_playback():
    publish_action(services().source.restart())
    return redirect(url_for("dashboard.dashboard"))


@dashboard_blueprint.post("/playback/clear")
@login_required
def clear_playback():
    publish_action(services().source.clear())
    return redirect(url_for("dashboard.dashboard"))
