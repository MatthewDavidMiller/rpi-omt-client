//! HDMI connector selection through sysfs. A connector name resolves to a
//! card device, a DRM connector id, and the matching ALSA device, and the
//! binding is re-checked on every hotplug poll so a card renumber cannot make
//! the receiver drive the wrong display.

use std::fs;
use std::path::{Path, PathBuf};

use omt_receiver_core::Connector;

const SYSFS_ROOT: &str = "/sys/class/drm";
const DEVICE_ROOT: &str = "/dev/dri";
const SOUND_ROOT: &str = "/sys/class/sound";

/// Connector names the appliance supports, in auto-selection order.
pub const SUPPORTED: [&str; 2] = ["HDMI-A-1", "HDMI-A-2"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hdmi {
    pub name: String,
    pub card_path: PathBuf,
    pub sysfs_path: PathBuf,
    pub id: u32,
    pub alsa_device: String,
}

/// Reads a small sysfs attribute, trimmed.
fn read_line(dir: &Path, file: &str) -> Option<String> {
    let text = omt_common::fsio::read_text(dir.join(file), 4096).ok()??;
    Some(text.trim().to_owned())
}

/// Finds a connector by name, or the first supported one for "auto".
pub fn find(preference: &str) -> Option<Hdmi> {
    let roots = (Path::new(SYSFS_ROOT), Path::new(DEVICE_ROOT), Path::new(SOUND_ROOT));
    if preference == "auto" {
        return SUPPORTED.iter().find_map(|name| named_in(roots.0, roots.1, roots.2, name));
    }
    if SUPPORTED.contains(&preference) {
        return named_in(roots.0, roots.1, roots.2, preference);
    }
    None
}

/// The selection over explicit roots, so it can be tested without a Pi.
pub fn named_in(
    sysfs_root: &Path,
    device_root: &Path,
    sound_root: &Path,
    name: &str,
) -> Option<Hdmi> {
    let suffix = format!("-{name}");
    // Several cards can expose the same connector name; the lowest-numbered
    // one that is actually connected wins, which keeps selection stable.
    let mut candidates: Vec<String> = fs::read_dir(sysfs_root)
        .ok()?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.len() > suffix.len() && n.starts_with("card") && n.ends_with(&suffix))
        .take(64)
        .collect();
    candidates.sort();
    // Every rejection disqualifies that card only; the next candidate may
    // still be the display the operator plugged in.
    candidates.into_iter().find_map(|candidate| {
        let sysfs_path = sysfs_root.join(&candidate);
        if read_line(&sysfs_path, "status")? != "connected" {
            return None;
        }
        let id = omt_common::buf::parse_u64(
            read_line(&sysfs_path, "connector_id")?.as_bytes(),
            u64::from(u32::MAX),
        )
        .and_then(|v| u32::try_from(v).ok())
        .filter(|&v| v != 0)?;
        let card = &candidate[..candidate.len() - suffix.len()];
        let card_path = device_root.join(card);
        if !card_path.exists() {
            return None;
        }
        Some(Hdmi {
            name: name.to_owned(),
            card_path,
            sysfs_path,
            id,
            alsa_device: alsa_device_in(sound_root, name),
        })
    })
}

/// Pi 4 and Pi 5 register one ALSA card per output (vc4hdmi0, vc4hdmi1);
/// Pi 3 and Zero 2 W register a single unindexed vc4hdmi, which on a
/// one-output board is the output. When the tree cannot be read the indexed
/// name is still reported, so the status names the device that was
/// attempted.
pub fn alsa_device_in(sound_root: &Path, name: &str) -> String {
    let preferred = if name == "HDMI-A-1" { "vc4hdmi0" } else { "vc4hdmi1" };
    let mut single = false;
    let mut has_preferred = false;
    if let Ok(entries) = fs::read_dir(sound_root) {
        for entry in entries.flatten() {
            if !entry.file_name().as_encoded_bytes().starts_with(b"card") {
                continue;
            }
            let Some(id) = read_line(&entry.path(), "id") else { continue };
            has_preferred |= id == preferred;
            single |= id == "vc4hdmi";
        }
    }
    if !has_preferred && single {
        return "plughw:CARD=vc4hdmi,DEV=0".to_owned();
    }
    format!("plughw:CARD={preferred},DEV=0")
}

impl Hdmi {
    pub fn is_connected(&self) -> bool {
        read_line(&self.sysfs_path, "status").as_deref() == Some("connected")
            && read_line(&self.sysfs_path, "connector_id") == Some(self.id.to_string())
    }

    pub fn describe(&self) -> Connector {
        Connector {
            name: self.name.clone(),
            drm_device: self.card_path.display().to_string(),
            alsa_device: self.alsa_device.clone(),
        }
    }
}
