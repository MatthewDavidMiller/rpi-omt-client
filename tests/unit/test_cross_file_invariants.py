"""Constants that have to agree across the container, host, and receiver.

Each of these is a value one file computes with and another file supplies. They
are joined by a comment today, which is exactly as strong as whoever reads it:
`settings.py` derives the diagnostics bundle ceiling from a Gunicorn timeout it
does not set, and the host diagnostics budget is spelled out in three places
that no test compares. A divergence is silent -- a bundle collected past the
worker timeout, or a host ceiling the support archive misreports -- so the
coupling is asserted here rather than described.
"""

from __future__ import annotations

import re

from conftest import REPO_ROOT

from omt_client.playback_status import CONNECTORS
from omt_client.settings import ENVIRONMENT_SPECS, GUNICORN_WORKER_TIMEOUT_SECONDS

ENTRYPOINT = REPO_ROOT / "deploy" / "container" / "entrypoint.sh"
INSTALLER = REPO_ROOT / "deploy" / "host" / "install.sh"
HOST_DIAGNOSTICS = REPO_ROOT / "deploy" / "host" / "host-diagnostics.sh"
START_OMT = REPO_ROOT / "deploy" / "container" / "start-omt.sh"
RECEIVER_MAIN = REPO_ROOT / "crates" / "omt-receiver" / "src" / "main.rs"


def _spec_default(name: str) -> str:
    return next(spec.default for spec in ENVIRONMENT_SPECS if spec.name == name)


def test_bundle_ceiling_matches_the_gunicorn_timeout_it_is_derived_from():
    """`load_settings` refuses a bundle budget that would outlive the worker."""
    entrypoint = ENTRYPOINT.read_text(encoding="utf-8")
    configured = re.search(r"--timeout (\d+)", entrypoint)
    assert configured is not None, "entrypoint.sh no longer sets a Gunicorn timeout"
    assert int(configured.group(1)) == GUNICORN_WORKER_TIMEOUT_SECONDS


def test_host_diagnostics_budget_agrees_across_every_file_that_states_it():
    """The container reports this ceiling; the host unit is what enforces it."""
    expected = _spec_default("OMT_DIAGNOSTICS_HOST_BUDGET_SECONDS")
    exported = re.search(
        r"^export OMT_DIAGNOSTICS_HOST_BUDGET_SECONDS=(\d+)$",
        INSTALLER.read_text(encoding="utf-8"),
        re.MULTILINE,
    )
    assert exported is not None, "install.sh no longer exports the host budget"
    assert exported.group(1) == expected

    fallback = re.search(
        r'DIAGNOSTICS_BUDGET_SECONDS="\$\{OMT_DIAGNOSTICS_HOST_BUDGET_SECONDS:-(\d+)\}"',
        HOST_DIAGNOSTICS.read_text(encoding="utf-8"),
    )
    assert fallback is not None, "host-diagnostics.sh no longer defaults the host budget"
    assert fallback.group(1) == expected


def test_every_layer_accepts_the_same_hdmi_connector_names():
    """A name the launcher accepts and the receiver rejects is a receiver that
    never starts; one the receiver publishes and the status contract rejects is
    a dashboard stuck on "status stale"."""
    launcher = re.search(r"\^\(auto\|([A-Za-z0-9|-]+)\)\$", START_OMT.read_text(encoding="utf-8"))
    assert launcher is not None, "start-omt.sh no longer validates the connector"
    launcher_names = set(launcher.group(1).split("|"))

    receiver = re.search(
        r'matches!\(\s*preference\.as_str\(\),\s*"auto"([^)]*)\)',
        RECEIVER_MAIN.read_text(encoding="utf-8"),
    )
    assert receiver is not None, "the receiver no longer restricts --connector"
    receiver_names = set(re.findall(r'"([A-Za-z0-9-]+)"', receiver.group(1)))

    assert launcher_names == receiver_names
    # "none" is the receiver's answer when no display is attached, so the
    # status contract carries it and the selectable names besides.
    assert CONNECTORS == launcher_names | {"none"}
