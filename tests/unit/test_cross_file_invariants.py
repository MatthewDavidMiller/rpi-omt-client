"""Constants that have to agree across the container, host, and receiver.

Each of these is a value one file computes with and another file supplies. They
are joined by a comment today, which is exactly as strong as whoever reads it:
The host diagnostics budget is spelled out in three places
that no test compares. A divergence is silent -- a bundle collected past the
worker timeout, or a host ceiling the support archive misreports -- so the
coupling is asserted here rather than described.
"""

from __future__ import annotations

import re

from conftest import REPO_ROOT

WEB_SETTINGS = REPO_ROOT / "crates" / "omt-web" / "src" / "settings.rs"
WEB_PLAYBACK = REPO_ROOT / "crates" / "omt-web" / "src" / "playback.rs"
WEB_STATE = REPO_ROOT / "crates" / "omt-web" / "src" / "state.rs"
INSTALLER = REPO_ROOT / "deploy" / "host" / "install.sh"
HOST_DIAGNOSTICS = REPO_ROOT / "deploy" / "host" / "host-diagnostics.sh"
START_OMT = REPO_ROOT / "deploy" / "container" / "start-omt.sh"
RECEIVER_MAIN = REPO_ROOT / "crates" / "omt-receiver" / "src" / "main.rs"


def test_host_diagnostics_budget_agrees_across_every_file_that_states_it():
    """The container reports this ceiling; the host unit is what enforces it."""
    configured = re.search(
        r'integer\("OMT_DIAGNOSTICS_HOST_BUDGET_SECONDS", (\d+), 1\)',
        WEB_SETTINGS.read_text(encoding="utf-8"),
    )
    assert configured is not None
    expected = configured.group(1)
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
    web_names = set(
        re.findall(
            r'\["none", "(HDMI-A-[12])", "(HDMI-A-[12])"\]',
            WEB_PLAYBACK.read_text(encoding="utf-8"),
        )[0]
    ) | {"none"}
    assert web_names == launcher_names | {"none"}


BOARD_PROFILE = REPO_ROOT / "deploy" / "lib" / "board-profile.sh"
RECEIVER_CORE = REPO_ROOT / "crates" / "omt-receiver-core" / "src" / "lib.rs"
DEPLOYER_OPS = REPO_ROOT / "crates" / "omt-deployer-core" / "src" / "ops.rs"


def _shell_ceilings() -> list[str]:
    """The literal tiers from the board table, not the validator's own `$1`."""
    return re.findall(
        r'ceiling="(\d[^"]*)"',
        BOARD_PROFILE.read_text(encoding="utf-8"),
    )


def test_every_shipped_board_ceiling_is_covered_by_the_rust_web_tests():
    ceilings = _shell_ceilings()
    assert len(ceilings) == 4, "board-profile.sh no longer defines four board tiers"
    assert "parse_video_ceiling" in WEB_STATE.read_text(encoding="utf-8")


def test_the_absolute_video_limits_agree_across_all_three_implementations():
    """Shell and the two Rust crates each bound a ceiling independently. They are what
    `omt-protocol` sizes its allocations for, so a layer that allowed more would
    promise what the decoder cannot deliver."""
    shell = BOARD_PROFILE.read_text(encoding="utf-8")
    rust = RECEIVER_CORE.read_text(encoding="utf-8")
    web = WEB_STATE.read_text(encoding="utf-8")
    for name, value in (
        ("WIDTH", 1920),
        ("HEIGHT", 1080),
        ("FPS", 60),
    ):
        assert f"HOST_ABSOLUTE_MAX_{name}={value}" in shell
        assert f"const CEILING_MAX_{name}: i32 = {value};" in rust
        assert str(value) in web
    assert "HOST_MAX_CEILING_SHAPES=4" in shell
    assert "const CEILING_MAX_SHAPES: usize = 4;" in rust
    assert "CEILING_MIN_DIMENSION: i32 = 16;" in rust


def test_the_supported_board_table_agrees_between_the_host_and_the_deployer():
    """`board-profile.sh` gates the install on the Pi; `ops.rs` and `deploy.sh`
    gate the upload from the workstation. A board only one of them accepts is a
    deployment that either refuses a supported Pi or uploads to a board the
    installer will then reject."""
    shell = BOARD_PROFILE.read_text(encoding="utf-8")
    rust_prefixes = re.search(
        r"const SUPPORTED_BOARDS: \[&str; 4\] = \[(.*?)\];",
        DEPLOYER_OPS.read_text(encoding="utf-8"),
        re.DOTALL,
    )
    assert rust_prefixes is not None, "ops.rs no longer lists the supported boards"
    for prefix in re.findall(r'"([^"]+)"', rust_prefixes.group(1)):
        assert f'"{prefix}"' in shell, f"{prefix} is accepted by the deployer but not the installer"

    # `make deploy` reuses the shell table rather than restating it, which is
    # what keeps that third gate from drifting on its own.
    deploy_script = (REPO_ROOT / "scripts" / "deploy.sh").read_text(encoding="utf-8")
    assert "board-profile.sh" in deploy_script
    assert "host_board_profile" in deploy_script


def test_docker_api_wait_agrees_between_the_installer_and_openrc():
    installer = INSTALLER.read_text(encoding="utf-8")
    openrc = (REPO_ROOT / "deploy" / "openrc" / "omt-client").read_text(encoding="utf-8")
    configured = re.search(r"^DOCKER_API_WAIT_SECONDS=(\d+)$", installer, re.MULTILINE)
    assert configured is not None, "install.sh no longer pins the Docker API wait"
    expected = configured.group(1)
    assert expected == "90"
    assert "OMT_DOCKER_API_WAIT_SECONDS=${DOCKER_API_WAIT_SECONDS}" in installer
    assert f"OMT_DOCKER_API_WAIT_SECONDS:-{expected}" in openrc


def test_playback_status_stale_floor_stays_above_the_receiver_heartbeat():
    heartbeat = re.search(
        r"pub const HEARTBEAT: Duration = Duration::from_millis\((\d+)\)",
        RECEIVER_CORE.read_text(encoding="utf-8"),
    )
    assert heartbeat is not None, "receiver-core no longer publishes HEARTBEAT"
    stale = re.search(
        r'"OMT_PLAYBACK_STATUS_STALE_SECONDS",\s*\n\s*(\d+),\s*\n\s*(\d+)',
        WEB_SETTINGS.read_text(encoding="utf-8"),
    )
    assert stale is not None, "web settings no longer pin the stale default and floor"
    default_seconds = int(stale.group(1))
    min_seconds = int(stale.group(2))
    heartbeat_ms = int(heartbeat.group(1))
    assert min_seconds * 1000 > heartbeat_ms
    assert default_seconds >= min_seconds
