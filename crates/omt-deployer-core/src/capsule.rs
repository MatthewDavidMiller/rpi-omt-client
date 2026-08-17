//! The manifest-v3 capsule, as compiled into this binary.
//!
//! The deployer carries the appliance rather than pointing at it: `build.rs`
//! embeds every member of `deploy/manifest-v3.txt`, the ARM64 image archive
//! included. An operator therefore runs one executable, with no checkout, no
//! archive to copy in beside it, and no way to pair a deployer of one release
//! with host scripts of another.
//!
//! A developer can still deploy from a working tree -- see
//! [`crate::DeployOptions::project_root`] -- which is the only path that reads
//! any of this from disk.

/// One file the Raspberry Pi receives, and the bytes that were compiled in.
pub struct CapsuleMember {
    /// The member's path within the capsule, exactly as the manifest names it.
    pub name: &'static str,
    pub bytes: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/capsule.rs"));

/// The appliance image archive.
///
/// Named rather than inferred: the archive is the one member whose handling
/// differs -- it is what a `--project` rebuild replaces -- and a rule like "the
/// member ending in .tar.gz" would quietly pick a second archive if one were
/// ever added. [`tests::the_image_member_is_embedded`] holds the manifest to it.
pub const IMAGE_MEMBER: &str = "omt-client-arm64.tar.gz";

/// Every member of the embedded capsule, in manifest order.
pub fn embedded_members() -> &'static [CapsuleMember] {
    MEMBERS
}

/// The embedded member called `name`.
pub fn embedded_member(name: &str) -> Option<&'static CapsuleMember> {
    MEMBERS.iter().find(|member| member.name == name)
}

/// The embedded appliance image archive.
pub fn embedded_image() -> Option<&'static CapsuleMember> {
    embedded_member(IMAGE_MEMBER)
}

#[cfg(test)]
mod tests {
    use super::{IMAGE_MEMBER, embedded_image, embedded_members};
    use crate::{parse_manifest, valid_manifest_name};

    /// The capsule this binary carries and the manifest it carries beside it
    /// are the same list. The Pi reads that manifest when it promotes the
    /// staged files, so a capsule that did not match it would promote a set
    /// nobody assembled.
    #[test]
    fn the_embedded_capsule_is_exactly_the_manifest_it_ships() {
        let manifest = embedded_members()
            .iter()
            .find(|member| member.name == "deploy/manifest-v3.txt")
            .map_or_else(
                || panic!("the capsule does not carry its own manifest"),
                |member| member.bytes,
            );
        let text = std::str::from_utf8(manifest).unwrap_or_else(|error| panic!("{error}"));
        let named = parse_manifest(text).unwrap_or_else(|error| panic!("{error}"));
        let embedded: Vec<&str> = embedded_members().iter().map(|m| m.name).collect();
        assert_eq!(named, embedded);
    }

    /// `build.rs` restates `valid_manifest_name` because a build script cannot
    /// depend on the crate it builds. This is the real rule run over what that
    /// copy accepted.
    #[test]
    fn every_embedded_name_is_a_safe_manifest_member() {
        for member in embedded_members() {
            assert!(
                valid_manifest_name(member.name),
                "unsafe embedded member: {}",
                member.name
            );
            assert!(!member.bytes.is_empty(), "empty member: {}", member.name);
        }
    }

    #[test]
    fn the_capsule_carries_what_the_pi_promotes_with() {
        for required in [
            "deploy/transaction.sh",
            "deploy/manifest-v3.txt",
            "deploy/host/install.sh",
            "deploy/host/bootstrap.sh",
            "deploy/host/setup-sys.sh",
        ] {
            assert!(
                embedded_members().iter().any(|m| m.name == required),
                "the capsule is missing {required}"
            );
        }
    }

    /// The archive is the reason this deployer needs nothing beside it, so
    /// "present" is not enough: a truncated or wrongly named build would
    /// otherwise be discovered on the Pi, after an upload.
    #[test]
    fn the_image_member_is_embedded_and_is_a_gzip_archive() {
        let image =
            embedded_image().unwrap_or_else(|| panic!("{IMAGE_MEMBER} is not in the capsule"));
        assert!(
            image.bytes.len() > 1024 * 1024,
            "the embedded appliance image is only {} bytes",
            image.bytes.len()
        );
        assert_eq!(
            &image.bytes[..2],
            &[0x1f, 0x8b],
            "the embedded appliance image is not gzip"
        );
    }
}
