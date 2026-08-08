"""Read-only diagnostic checks and support-bundle routes."""

from __future__ import annotations

from flask import Blueprint, render_template, request, send_file
from flask.typing import ResponseReturnValue

from ..models import DiagnosticResult
from .common import login_required, services

diagnostics_blueprint = Blueprint("diagnostics", __name__)


def _render(result: DiagnosticResult | None = None) -> ResponseReturnValue:
    container = services()
    configuration = container.source.configuration()
    return render_template(
        "diagnostics.html",
        app_version=container.about.version(),
        current_source=configuration.source,
        current_direct_target=configuration.direct_address,
        configuration_error=configuration.error,
        omt_status=container.diagnostics.status(),
        result=result,
    )


@diagnostics_blueprint.get("/diagnostics")
@login_required
def diagnostics() -> ResponseReturnValue:
    return _render()


@diagnostics_blueprint.post("/diagnostics/discovery")
@login_required
def discovery_check() -> ResponseReturnValue:
    services().source.refresh()
    return _render(services().diagnostics.discovery())


@diagnostics_blueprint.post("/diagnostics/runtime")
@login_required
def runtime_check() -> ResponseReturnValue:
    return _render(services().diagnostics.runtime())


@diagnostics_blueprint.post("/diagnostics/direct")
@login_required
def direct_check() -> ResponseReturnValue:
    return _render(services().diagnostics.direct(request.form.get("direct_address", "").strip()))


@diagnostics_blueprint.post("/diagnostics/download")
@login_required
def download_bundle() -> ResponseReturnValue:
    include_packet_capture = request.form.get("include_packet_capture") == "1"
    bundle, filename = services().diagnostics.bundle(include_packet_capture)
    return send_file(
        bundle,
        mimetype="application/zip",
        as_attachment=True,
        download_name=filename,
    )
