"""Authenticated product and legal information."""

from flask import Blueprint, current_app, render_template

from ..safe_io import read_text
from .common import login_required, services

about_blueprint = Blueprint("about", __name__)
LEGAL_FILE_LIMIT = 2 * 1024 * 1024


def _legal_text(path: str, label: str) -> str:
    result = read_text(path, LEGAL_FILE_LIMIT)
    if result.ok:
        return result.text
    current_app.logger.error(
        "Unable to load %s from %s: %s",
        label,
        path,
        result.detail or result.status.value,
    )
    return f"{label} is unavailable in this image."


@about_blueprint.get("/about")
@login_required
def about():
    settings = current_app.extensions["omt_client.settings"]
    return render_template(
        "about.html",
        app_version=services().diagnostics.version(),
        project_license=_legal_text(settings.project_license_file, "Project license"),
        third_party_notices=_legal_text(settings.third_party_notices_file, "Third-party notices"),
    )
