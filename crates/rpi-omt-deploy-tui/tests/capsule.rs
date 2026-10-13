//! The capsule a deployer actually ships, held to the manifest it carries,
//! ported from tests/c/test_deploy_capsule.c. It needs the ARM64 image
//! built first, so the deployer gate runs it with OMT_CAPSULE_REQUIRED=1;
//! without that, a build that embedded nothing skips it rather than
//! failing the workspace suites.

#![allow(clippy::unwrap_used)]

use omt_capsule::CAPSULE;
use omt_deploy::core;
use omt_deploy::ops;
use omt_tui::ui;

fn embedded() -> bool {
    if CAPSULE.members.is_empty() && option_env!("OMT_CAPSULE_REQUIRED") != Some("1") {
        eprintln!("skipped: this build embedded no capsule (set OMT_CAPSULE_REQUIRED=1)");
        return false;
    }
    true
}

fn text(name: &str) -> &'static str {
    std::str::from_utf8(CAPSULE.member_named(name).unwrap().bytes).unwrap()
}

#[test]
fn the_embedded_capsule_is_exactly_the_manifest_it_ships() {
    if !embedded() {
        return;
    }
    let names = core::parse_manifest(text("deploy/manifest-v3.txt")).unwrap();
    let members: Vec<&str> = CAPSULE.members.iter().map(|m| m.name).collect();
    assert_eq!(names, members);
}

/// The build script restates core::valid_manifest_name; this is the real
/// rule run over what it accepted.
#[test]
fn every_embedded_name_is_a_safe_manifest_member() {
    if !embedded() {
        return;
    }
    for m in CAPSULE.members {
        assert!(core::valid_manifest_name(m.name), "unsafe: {}", m.name);
        assert!(!m.bytes.is_empty(), "empty: {}", m.name);
    }
}

#[test]
fn the_capsule_carries_what_the_pi_promotes_with() {
    if !embedded() {
        return;
    }
    for required in [
        "deploy/transaction.sh",
        "deploy/manifest-v3.txt",
        "deploy/host/install.sh",
        "deploy/host/bootstrap.sh",
        "deploy/host/setup-sys.sh",
    ] {
        assert!(CAPSULE.member_named(required).is_some(), "missing {required}");
    }
}

/// The archive is why this deployer needs nothing beside it, so "present"
/// is not enough: a truncated or misnamed build would surface on the Pi.
#[test]
fn the_image_member_is_embedded_and_is_a_gzip_archive() {
    if !embedded() {
        return;
    }
    let image = CAPSULE.image().unwrap();
    assert!(image.bytes.len() > 1024 * 1024, "only {} bytes", image.bytes.len());
    assert!(image.bytes.starts_with(&[0x1f, 0x8b]));
}

/// The deployer waits for a marker the script prints and uploads a member
/// the capsule has to contain; both are checked against the embedded bytes.
#[test]
fn the_embedded_rename_script_is_the_one_ops_drives() {
    if !embedded() {
        return;
    }
    let script = text(ops::SET_HOSTNAME_MEMBER);
    assert!(script.contains(ops::SET_HOSTNAME_COMPLETE));
    assert!(script.contains("rc-service omt-client restart"));
    assert!(text(ops::SETUP_SYS_MEMBER).contains(ops::SETUP_SYS_COMPLETE));
}

#[test]
fn about_reproduces_the_shipped_licence_and_notices() {
    if !embedded() {
        return;
    }
    let mut lines = Vec::new();
    ui::about_text(&CAPSULE, 80, &mut lines);
    let joined = lines.join("\n");
    for expected in [
        "MIT License",
        "Copyright (c) 2026 Matthew David Miller",
        "Permission is hereby granted, free of charge",
        "THIRD-PARTY NOTICES",
        "SPDX license identifiers above are descriptive",
    ] {
        assert!(joined.contains(expected), "missing {expected}");
    }
    assert!(CAPSULE.report().unwrap().contains("sha256 "));
}
