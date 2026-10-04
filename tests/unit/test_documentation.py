import re

from conftest import REPO_ROOT


def test_local_markdown_links_resolve():
    pattern = re.compile(r"\[[^]]*]\((?!https?://|#|mailto:)([^)#]+)(?:#[^)]+)?\)")
    missing = []
    for document in [REPO_ROOT / "README.md", *(REPO_ROOT / "docs").glob("*.md")]:
        for target in pattern.findall(document.read_text(encoding="utf-8")):
            if not (document.parent / target).resolve().exists():
                missing.append(f"{document.relative_to(REPO_ROOT)} -> {target}")
    assert not missing, "Missing documentation targets: " + ", ".join(missing)


def test_public_application_settings_are_documented():
    configuration = (REPO_ROOT / "docs" / "CONFIGURATION.md").read_text(encoding="utf-8")
    settings_source = (REPO_ROOT / "src" / "web" / "settings.c").read_text(encoding="utf-8")
    public_names = {
        name
        for name in re.findall(r'"(OMT_[A-Z0-9_]+)"', settings_source)
        if not name.endswith("_")
    }
    missing = [name for name in sorted(public_names) if f"`{name}`" not in configuration]
    assert not missing


def test_deployment_manifest_names_existing_files():
    names = (REPO_ROOT / "deploy" / "manifest-v3.txt").read_text(encoding="ascii").splitlines()
    assert names.pop(0) == "version=3"
    assert names
    assert all(name == "omt-client-arm64.tar.gz" or (REPO_ROOT / name).is_file() for name in names)


def test_high_value_paths_are_documented():
    """Existence of every backticked path is covered generically below; this only
    pins that the file map still mentions the entry points worth documenting."""
    reference = (REPO_ROOT / "docs" / "CODEBASE_REFERENCE.md").read_text(encoding="utf-8")
    paths = (
        "src/web/app.c",
        "src/web/auth.c",
        "src/web/state.c",
        "deploy/container/runtime-lib.sh",
        "deploy/container/entrypoint.sh",
        "src/receiver/main.c",
        "src/protocol/omt.c",
        "src/deploy/core/ops.c",
        "src/deploy/ssh/kex.c",
        "src/deploy/cli/main.c",
        "src/deploy/tui/app.c",
        "deploy/manifest-v3.txt",
        "deploy/transaction.sh",
    )
    assert all(f"`{path}`" in reference for path in paths)


def test_file_maps_reference_existing_paths():
    """The agent guides, the development entry point, and the two architecture
    documents are the map for humans and agents alike. Their file maps silently
    rotted through the v0.9.27 src/ + deploy/ reorganisation; this keeps every
    backticked repo path honest.

    CONFIGURATION.md and OPERATIONS.md are excluded: they name container runtime
    paths that deliberately do not exist in the repository."""
    path_pattern = re.compile(r"`([A-Za-z0-9_.][A-Za-z0-9_./-]*/[A-Za-z0-9_./-]*)`")
    guides = (
        REPO_ROOT / "AGENTS.md",
        REPO_ROOT / "CLAUDE.md",
        REPO_ROOT / "docs" / "DEVELOPMENT.md",
        REPO_ROOT / "docs" / "CODEBASE_REFERENCE.md",
        REPO_ROOT / "docs" / "ARCHITECTURE.md",
    )
    missing = []
    for guide in guides:
        for candidate in path_pattern.findall(guide.read_text(encoding="utf-8")):
            if candidate.endswith("/*") or "<" in candidate:
                continue
            if not (REPO_ROOT / candidate.rstrip("/")).exists():
                missing.append(f"{guide.name} -> {candidate}")
    assert not missing, "File maps name paths that do not exist: " + ", ".join(missing)


def test_public_factory_routes_are_documented():
    app_source = (REPO_ROOT / "src" / "web" / "app.c").read_text(encoding="utf-8")
    # The route table: one {"<path>", get_handler, post_handler} row per route.
    public_routes = {
        route
        for route in re.findall(r'^\s*\{"(/[^"]*)",', app_source, re.MULTILINE)
        if not route.startswith("/static/")
    }
    assert public_routes, "src/web/app.c no longer has a recognisable route table"
    route_docs = "\n".join(
        (REPO_ROOT / "docs" / name).read_text(encoding="utf-8")
        for name in ("CODEBASE_REFERENCE.md", "OPERATIONS.md")
    )
    missing = [route for route in sorted(public_routes) if route not in route_docs]
    assert not missing


def test_manifest_v3_nested_capsule_boundary_is_documented():
    manifest_names = (
        (REPO_ROOT / "deploy" / "manifest-v3.txt").read_text(encoding="ascii").splitlines()[1:]
    )
    architecture = (REPO_ROOT / "docs" / "ARCHITECTURE.md").read_text(encoding="utf-8")
    setup = (REPO_ROOT / "docs" / "SETUP.md").read_text(encoding="utf-8")
    assert "manifest version 3" in architecture
    assert "nested paths" in architecture
    assert all(name in setup or name == "omt-client-arm64.tar.gz" for name in manifest_names)


def test_headless_wifi_example_contains_the_required_fields():
    setup = (REPO_ROOT / "docs" / "SETUP.md").read_text(encoding="utf-8")
    block = re.search(r"```ini\n(.*?)```", setup, re.DOTALL)
    assert block is not None
    example = block.group(1)
    assert re.search(r"^country=[A-Z]{2}$", example, re.MULTILINE)
    assert "network={" in example
    assert "ssid=<hex-encoded-network-name>" in example
    assert "psk=<64-character-derived-psk>" in example
    assert "key_mgmt=WPA-PSK" in example


def test_first_use_documents_web_password_retrieval():
    setup = (REPO_ROOT / "docs" / "SETUP.md").read_text(encoding="utf-8")
    operations = (REPO_ROOT / "docs" / "OPERATIONS.md").read_text(encoding="utf-8")
    for document in (setup, operations):
        assert "Web UI password" in document
        assert "/etc/conf.d/omt-client" in document
        assert "logs omt-client" in document
        assert "PBKDF2" in document or "one-way hash" in document


def test_deployer_web_password_rotation_is_documented():
    setup = (REPO_ROOT / "docs" / "SETUP.md").read_text(encoding="utf-8")
    operations = (REPO_ROOT / "docs" / "OPERATIONS.md").read_text(encoding="utf-8")
    configuration = (REPO_ROOT / "docs" / "CONFIGURATION.md").read_text(encoding="utf-8")
    tui = (REPO_ROOT / "src" / "deploy" / "tui" / "app.c").read_text(encoding="utf-8")
    # The labels the operator is told to look for are the ones the view draws.
    for label in ("Also set the Web GUI password", "Change Web GUI password"):
        assert f'"{label}"' in tui
        assert label in setup
    assert "web-password" in operations
    assert "12-128" in operations
    assert "SSH stdin" in operations
    assert "off by default" in configuration
    assert "invalidating existing sessions" in configuration
