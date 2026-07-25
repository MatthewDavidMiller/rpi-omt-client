"""OMT Discovery Server and direct-target settings routes."""

from __future__ import annotations

from flask import Blueprint, redirect, render_template, request, url_for
from flask.typing import ResponseReturnValue

from .common import login_required, publish_action, services

network_blueprint = Blueprint("network", __name__)


@network_blueprint.route("/settings/network", methods=["GET", "POST"])
@login_required
def network_settings() -> ResponseReturnValue:
    submitted: dict[str, str] | None = None
    if request.method == "POST":
        submitted = {
            "discovery_server_text": request.form.get("discovery_server", ""),
        }
        result = services().network.save(submitted["discovery_server_text"])
        publish_action(result)
        if result.ok:
            return redirect(url_for("network.network_settings"))
    configuration = services().network.read()
    if submitted is not None:
        configuration.update(submitted)
    current_source, current_direct_target = services().source.configuration()
    return render_template(
        "network.html",
        network=configuration,
        current_source=current_source,
        current_direct_target=current_direct_target,
    )


@network_blueprint.post("/settings/direct-source")
@login_required
def save_direct_source() -> ResponseReturnValue:
    result = services().source.save_direct(request.form.get("direct_address", "").strip())
    publish_action(result)
    return redirect(url_for("network.network_settings"))
