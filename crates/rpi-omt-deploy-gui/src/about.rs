//! The About document: the licence and the third-party notices, which the
//! single-file deployer is the only copy of an operator receives.

use std::fmt::Write as _;

use omt_common::VERSION;
use omt_deploy::capsule::Capsule;
use omt_deploy::core::IMAGE_MEMBER;

fn trim_tail(text: &str) -> &str {
    text.trim_end_matches(['\n', ' ', '\t'])
}

/// What this deployer is and carries: its version and the digest of the
/// appliance build inside it.
pub fn summary(capsule: &Capsule) -> String {
    let mut out = format!("Raspberry Pi OMT client deployer {VERSION}\n\n");
    // The digest is the point: a single-file deployer is otherwise opaque
    // about which appliance build it carries.
    match capsule.report() {
        Ok(report) => out.push_str(trim_tail(&report)),
        Err(_) => {
            let _ = write!(out, "This deployer was built without {IMAGE_MEMBER}.");
        }
    }
    out.push_str("\n\nEverything this deploys is compiled in; no checkout is needed.");
    out
}

pub fn license(capsule: &Capsule) -> &'static str {
    trim_tail(capsule.license)
}

pub fn notices(capsule: &Capsule) -> &'static str {
    trim_tail(capsule.notices)
}
