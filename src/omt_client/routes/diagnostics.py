"""Read-only diagnostic checks and support-bundle routes."""

from __future__ import annotations

from flask import Blueprint, render_template, request, send_file
from flask.typing import ResponseReturnValue

from ..models import DiagnosticResult
from .common import login_required, services

diagnostics_blueprint = Blueprint("diagnostics", __name__)


def _render(
    result: DiagnosticResult | None = None,
    omt_status: str | None = None,
) -> ResponseReturnValue:
    """Render the diagnostics page, reusing a controller answer the caller has.

    The page header shows `control-omt.sh status`. The runtime check already
    runs that command -- once, deliberately, so the two members it produces
    cannot contradict each other -- so asking again here spent a second flock
    and /proc walk to obtain a *third* observation, free to disagree with the
    one shown right beside it.
    """
    container = services()
    configuration = container.source.configuration()
    return render_template(
        "diagnostics.html",
        app_version=container.about.version(),
        current_source=configuration.source,
        current_direct_target=configuration.direct_address,
        configuration_error=configuration.error,
        omt_status=container.diagnostics.status() if omt_status is None else omt_status,
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
    result, omt_status = services().diagnostics.runtime()
    return _render(result, omt_status)


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
