import re
from pathlib import Path

from omt_client import create_app
from omt_client.preview import preview_services
from settings import ENVIRONMENT_SPECS, load_settings

ROOT = Path(__file__).resolve().parents[2]


def test_local_markdown_links_resolve():
    pattern = re.compile(r"\[[^]]*]\((?!https?://|#|mailto:)([^)#]+)(?:#[^)]+)?\)")
    missing = []
    for document in [ROOT / "README.md", *(ROOT / "docs").glob("*.md")]:
        for target in pattern.findall(document.read_text(encoding="utf-8")):
            if not (document.parent / target).resolve().exists():
                missing.append(f"{document.relative_to(ROOT)} -> {target}")
    assert not missing, "Missing documentation targets: " + ", ".join(missing)


def test_public_application_settings_are_documented():
    configuration = (ROOT / "docs" / "CONFIGURATION.md").read_text(encoding="utf-8")
    settings_source = (ROOT / "app" / "settings.py").read_text(encoding="utf-8")
    public_names = {spec.name for spec in ENVIRONMENT_SPECS}
    public_names.update(re.findall(r'env\.get\("([A-Z][A-Z0-9_]+)"', settings_source))
    missing = [name for name in sorted(public_names) if f"`{name}`" not in configuration]
    assert not missing


def test_deployment_manifest_names_existing_files():
    names = (ROOT / "deploy-artifacts.txt").read_text(encoding="ascii").splitlines()
    assert names
    assert all(name == "omt-client-arm64.tar.gz" or (ROOT / name).is_file() for name in names)


def test_high_value_paths_are_documented_and_exist():
    reference = (ROOT / "docs" / "CODEBASE_REFERENCE.md").read_text(encoding="utf-8")
    paths = (
        "app/omt_client/factory.py",
        "app/omt_client/services.py",
        "app/state_store.py",
        "omt/runtime-lib.sh",
        "omt/entrypoint.sh",
        "deployer/RpiOmt.Deployer.Core/Models.cs",
        "deployer/RpiOmt.Deployer.Core/ActionController.cs",
        "deployer/RpiOmt.Deployer.Core/DeploymentOperations.cs",
        "deploy-artifacts.txt",
        "deploy-transaction.sh",
    )
    assert all((ROOT / path).exists() and f"`{path}`" in reference for path in paths)


def test_public_factory_routes_are_documented():
    application = create_app(load_settings({}), preview_services())
    public_routes = {
        rule.rule for rule in application.url_map.iter_rules() if rule.endpoint != "static"
    }
    route_docs = "\n".join(
        (ROOT / "docs" / name).read_text(encoding="utf-8")
        for name in ("CODEBASE_REFERENCE.md", "OPERATIONS.md")
    )
    missing = [route for route in sorted(public_routes) if route not in route_docs]
    assert not missing


def test_manifest_contract_and_flat_capsule_boundary_are_documented():
    manifest_names = (ROOT / "deploy-artifacts.txt").read_text(encoding="ascii").splitlines()
    architecture = (ROOT / "docs" / "ARCHITECTURE.md").read_text(encoding="utf-8")
    setup = (ROOT / "docs" / "SETUP.md").read_text(encoding="utf-8")
    assert "nine-file capsule" in architecture
    assert "flat and self-contained" in architecture
    assert all(name in setup or name == "omt-client-arm64.tar.gz" for name in manifest_names)
