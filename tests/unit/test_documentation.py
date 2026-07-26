import re

from conftest import REPO_ROOT

from omt_client import create_app
from omt_client.settings import ENVIRONMENT_SPECS, RATE_LIMIT_SPECS, load_settings
from omt_client_preview import preview_services


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
    settings_source = (REPO_ROOT / "src" / "omt_client" / "settings.py").read_text(encoding="utf-8")
    public_names = {spec.name for spec in (*ENVIRONMENT_SPECS, *RATE_LIMIT_SPECS)}
    public_names.update(re.findall(r'env\.get\("([A-Z][A-Z0-9_]+)"', settings_source))
    missing = [name for name in sorted(public_names) if f"`{name}`" not in configuration]
    assert not missing


def test_deployment_manifest_names_existing_files():
    names = (REPO_ROOT / "deploy" / "manifest-v2.txt").read_text(encoding="ascii").splitlines()
    assert names.pop(0) == "version=2"
    assert names
    assert all(name == "omt-client-arm64.tar.gz" or (REPO_ROOT / name).is_file() for name in names)


def test_high_value_paths_are_documented():
    """Existence of every backticked path is covered generically below; this only
    pins that the file map still mentions the entry points worth documenting."""
    reference = (REPO_ROOT / "docs" / "CODEBASE_REFERENCE.md").read_text(encoding="utf-8")
    paths = (
        "src/omt_client/factory.py",
        "src/omt_client/services/composition.py",
        "src/omt_client/state_store.py",
        "deploy/container/runtime-lib.sh",
        "deploy/container/entrypoint.sh",
        "src/receiver/RpiOmt.Receiver.Core",
        "src/deployer/RpiOmt.Deployer.Core/Models.cs",
        "src/deployer/RpiOmt.Deployer.Core/ActionController.cs",
        "src/deployer/RpiOmt.Deployer.Core/DeploymentOperations.cs",
        "deploy/manifest-v2.txt",
        "deploy/transaction.sh",
    )
    assert all(f"`{path}`" in reference for path in paths)


def test_file_maps_reference_existing_paths():
    """The agent guides and the two architecture documents are the entry map for
    humans and agents. Their file maps silently rotted through the v0.9.27 src/ +
    deploy/ reorganisation; this keeps every backticked repo path honest.

    CONFIGURATION.md and OPERATIONS.md are excluded: they name container runtime
    paths that deliberately do not exist in the repository."""
    path_pattern = re.compile(r"`([A-Za-z0-9_.][A-Za-z0-9_./-]*/[A-Za-z0-9_./-]*)`")
    guides = (
        REPO_ROOT / "AGENTS.md",
        REPO_ROOT / "CLAUDE.md",
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
    application = create_app(load_settings({}), preview_services())
    public_routes = {
        rule.rule for rule in application.url_map.iter_rules() if rule.endpoint != "static"
    }
    route_docs = "\n".join(
        (REPO_ROOT / "docs" / name).read_text(encoding="utf-8")
        for name in ("CODEBASE_REFERENCE.md", "OPERATIONS.md")
    )
    missing = [route for route in sorted(public_routes) if route not in route_docs]
    assert not missing


def test_manifest_v2_nested_capsule_boundary_is_documented():
    manifest_names = (
        (REPO_ROOT / "deploy" / "manifest-v2.txt").read_text(encoding="ascii").splitlines()[1:]
    )
    architecture = (REPO_ROOT / "docs" / "ARCHITECTURE.md").read_text(encoding="utf-8")
    setup = (REPO_ROOT / "docs" / "SETUP.md").read_text(encoding="utf-8")
    assert "manifest version 2" in architecture
    assert "nested paths" in architecture
    assert all(name in setup or name == "omt-client-arm64.tar.gz" for name in manifest_names)
