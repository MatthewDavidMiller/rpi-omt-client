//! The manifest-v3 capsule a deployer carries.
//!
//! The deployer carries the appliance rather than pointing at it: the
//! binaries embed every member of deploy/manifest-v3.txt, the ARM64 image
//! archive included (crates/omt-capsule). An operator therefore runs one
//! executable, with no checkout, no archive to copy in beside it, and no way
//! to pair a deployer of one release with host scripts of another. The core
//! takes the capsule as a value, so the rest of the deployer builds and
//! tests without the image.

use omt_common::{Error, Result};

use crate::core::{IMAGE_MEMBER, sha256_hex};

#[derive(Debug)]
pub struct Member {
    pub name: &'static str,
    pub bytes: &'static [u8],
}

#[derive(Debug)]
pub struct Capsule {
    pub members: &'static [Member],
    /// The LICENSE and THIRD_PARTY_NOTICES.txt texts the release ships.
    pub license: &'static str,
    pub notices: &'static str,
}

impl Capsule {
    /// A capsule with nothing in it, for tools that never deploy.
    pub const EMPTY: Capsule = Capsule { members: &[], license: "", notices: "" };

    pub fn member_named(&self, name: &str) -> Option<&'static Member> {
        self.members.iter().find(|m| m.name == name)
    }

    /// Named rather than inferred: the archive is the one member whose
    /// handling differs, and a rule like "the member ending in .tar.gz"
    /// would quietly pick a second archive if one were ever added.
    pub fn image(&self) -> Option<&'static Member> {
        self.member_named(IMAGE_MEMBER)
    }

    /// "Embedded capsule: N members, omt-client-arm64.tar.gz M MiB, sha256 H."
    pub fn report(&self) -> Result<String> {
        let image = self
            .image()
            .ok_or_else(|| Error::new(format!("this deployer was built without {IMAGE_MEMBER}")))?;
        Ok(format!(
            "Embedded capsule: {} members, {IMAGE_MEMBER} {} MiB, sha256 {}.",
            self.members.len(),
            image.bytes.len() / (1024 * 1024),
            sha256_hex(image.bytes)
        ))
    }
}
