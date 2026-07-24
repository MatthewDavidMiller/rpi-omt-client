"""Read-only diagnostic checks and support-bundle routes."""

from flask import Blueprint, render_template, request, send_file

from .common import login_required, services

diagnostics_blueprint = Blueprint("diagnostics", __name__)


def _render(result=None):
    source, address = services().source.configuration()
    return render_template(
        "diagnostics.html",
        app_version=services().diagnostics.version(),
        current_source=source,
        current_source_url_address=address,
        omt_status=services().diagnostics.status(),
        result=result,
    )


@diagnostics_blueprint.get("/diagnostics")
@login_required
def diagnostics():
    return _render()


@diagnostics_blueprint.post("/diagnostics/discovery")
@login_required
def discovery_check():
    services().source.refresh()
    return _render(services().diagnostics.discovery())


@diagnostics_blueprint.post("/diagnostics/runtime")
@login_required
def runtime_check():
    return _render(services().diagnostics.runtime())


@diagnostics_blueprint.post("/diagnostics/direct")
@login_required
def direct_check():
    return _render(
        services().diagnostics.direct(request.form.get("direct_address", "").strip())
    )


@diagnostics_blueprint.post("/diagnostics/download")
@login_required
def download_bundle():
    include_packet_capture = request.form.get("include_packet_capture") == "1"
    bundle, filename = services().diagnostics.bundle(include_packet_capture)
    return send_file(
        bundle,
        mimetype="application/zip",
        as_attachment=True,
        download_name=filename,
    )
