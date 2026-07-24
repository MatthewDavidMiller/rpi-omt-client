"""Gunicorn application target."""

from .factory import create_app

app = create_app()
