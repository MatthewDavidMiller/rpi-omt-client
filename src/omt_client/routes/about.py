"""Authenticated product and legal information."""

from __future__ import annotations

from flask import Blueprint, render_template
from flask.typing import ResponseReturnValue

from .common import login_required, services

about_blueprint = Blueprint("about", __name__)


@about_blueprint.get("/about")
@login_required
def about() -> ResponseReturnValue:
    about_service = services().about
    license_text, notices_text = about_service.legal_texts()
    return render_template(
        "about.html",
        app_version=about_service.version(),
        project_license=license_text,
        third_party_notices=notices_text,
    )
