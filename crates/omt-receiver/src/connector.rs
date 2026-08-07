// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// HDMI connector selection through sysfs. The receiver resolves a connector
// name to a card device, a DRM connector id, and the matching ALSA device, and
// re-checks that binding on every hotplug poll so a card renumber cannot make
// the receiver drive the wrong display.

use std::fs;
use std::path::{Path, PathBuf};

const SYSFS_ROOT: &str = "/sys/class/drm";
/// Connector names the appliance supports, in auto-selection order.
pub const SUPPORTED: [&str; 2] = ["HDMI-A-1", "HDMI-A-2"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Connector {
    pub name: String,
    pub card_path: PathBuf,
    pub sysfs_path: PathBuf,
    pub id: u32,
    pub alsa_device: String,
}

impl Connector {
    /// The Pi 5's two HDMI outputs each carry their own ALSA card.
    fn alsa_device_for(name: &str) -> &'static str {
        if name == "HDMI-A-1" {
            "plughw:CARD=vc4hdmi0,DEV=0"
        } else {
            "plughw:CARD=vc4hdmi1,DEV=0"
        }
    }

    /// Re-reads sysfs to confirm the display is still attached and still has
    /// the connector id this binding was built from.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        read_line(&self.sysfs_path.join("status")).is_some_and(|status| status == "connected")
            && read_line(&self.sysfs_path.join("connector_id"))
                .is_some_and(|id| id == self.id.to_string())
    }

    /// Projects the connector into the status document's shape.
    #[must_use]
    pub fn describe(&self) -> omt_receiver_core::Connector {
        omt_receiver_core::Connector {
            name: self.name.clone(),
            drm_device: self.card_path.to_string_lossy().into_owned(),
            alsa_device: self.alsa_device.clone(),
        }
    }
}

/// Finds a connector by name, or the first supported one for `auto`.
#[must_use]
pub fn find(preference: &str) -> Option<Connector> {
    if preference == "auto" {
        return SUPPORTED.iter().find_map(|name| named(name));
    }
    named(preference)
}

fn named(name: &str) -> Option<Connector> {
    // Several cards can expose the same connector name; the lowest-numbered
    // card that is actually connected wins, which keeps selection stable.
    let mut candidates: Vec<String> = fs::read_dir(SYSFS_ROOT)
        .ok()?
        .filter_map(|entry| {
            let entry = entry.ok()?.file_name().into_string().ok()?;
            (entry.starts_with("card") && entry.ends_with(&format!("-{name}"))).then_some(entry)
        })
        .collect();
    candidates.sort();

    for entry in candidates {
        let sysfs_path = Path::new(SYSFS_ROOT).join(&entry);
        if read_line(&sysfs_path.join("status"))? != "connected" {
            continue;
        }
        let Some(id) = read_line(&sysfs_path.join("connector_id"))
            .and_then(|text| text.parse::<u32>().ok())
            .filter(|id| *id != 0)
        else {
            continue;
        };
        let card = entry.strip_suffix(&format!("-{name}"))?;
        let card_path = PathBuf::from("/dev/dri").join(card);
        if !card_path.exists() {
            continue;
        }
        return Some(Connector {
            name: name.to_owned(),
            card_path,
            sysfs_path,
            id,
            alsa_device: Connector::alsa_device_for(name).to_owned(),
        });
    }
    None
}

fn read_line(path: &Path) -> Option<String> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > 4096 {
        return None;
    }
    Some(fs::read_to_string(path).ok()?.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_each_output_to_its_own_audio_card() {
        assert_eq!(
            Connector::alsa_device_for("HDMI-A-1"),
            "plughw:CARD=vc4hdmi0,DEV=0"
        );
        assert_eq!(
            Connector::alsa_device_for("HDMI-A-2"),
            "plughw:CARD=vc4hdmi1,DEV=0"
        );
    }

    #[test]
    fn an_unknown_connector_is_never_selected() {
        assert!(find("HDMI-A-9").is_none());
        assert!(find("../../etc").is_none());
    }
}
