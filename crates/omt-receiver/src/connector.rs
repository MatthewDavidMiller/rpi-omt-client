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
const DEVICE_ROOT: &str = "/dev/dri";
const SOUND_ROOT: &str = "/sys/class/sound";
/// Connector names the appliance supports, in auto-selection order.
///
/// The Pi 3 and Zero 2 W expose only `HDMI-A-1`; on those boards `HDMI-A-2`
/// simply never resolves, which the play loop already reads as "no display
/// connected" rather than as an error.
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
    named_in(
        Path::new(SYSFS_ROOT),
        Path::new(DEVICE_ROOT),
        Path::new(SOUND_ROOT),
        name,
    )
}

/// The selection itself, over the two directory trees it reads rather than over
/// the absolute paths, so the multi-card fallback can be tested without a Pi.
///
/// Every rejection is `continue`, never an early return: an unreadable or
/// half-populated entry disqualifies *that card*, and the next candidate is
/// still the display the operator has plugged in. Returning from the whole
/// function on one unreadable `status` reported "no display connected" for a
/// connector that was attached to a later card.
fn named_in(
    sysfs_root: &Path,
    device_root: &Path,
    sound_root: &Path,
    name: &str,
) -> Option<Connector> {
    let suffix = format!("-{name}");
    // Several cards can expose the same connector name; the lowest-numbered
    // card that is actually connected wins, which keeps selection stable.
    let mut candidates: Vec<String> = fs::read_dir(sysfs_root)
        .ok()?
        .filter_map(|entry| {
            let entry = entry.ok()?.file_name().into_string().ok()?;
            (entry.starts_with("card") && entry.ends_with(&suffix)).then_some(entry)
        })
        .collect();
    candidates.sort();

    for entry in candidates {
        let sysfs_path = sysfs_root.join(&entry);
        if read_line(&sysfs_path.join("status")).as_deref() != Some("connected") {
            continue;
        }
        let Some(id) = read_line(&sysfs_path.join("connector_id"))
            .and_then(|text| text.parse::<u32>().ok())
            .filter(|id| *id != 0)
        else {
            continue;
        };
        let Some(card) = entry.strip_suffix(&suffix) else {
            continue;
        };
        let card_path = device_root.join(card);
        if !card_path.exists() {
            continue;
        }
        return Some(Connector {
            name: name.to_owned(),
            card_path,
            sysfs_path,
            id,
            alsa_device: alsa_device_in(sound_root, name),
        });
    }
    None
}

/// Resolves the HDMI output's ALSA card by reading the registered card ids.
///
/// The card layout is not the same on every supported board, and guessing it
/// from the connector name alone is what made HDMI audio fail silently on the
/// single-output boards:
///
/// * Pi 4 and Pi 5 register one card per output, `vc4hdmi0` and `vc4hdmi1`.
/// * Pi 3 and Zero 2 W have one HDMI and register a single card, `vc4hdmi`,
///   with no index at all.
///
/// So the id the connector would like is looked for first, and a lone
/// `vc4hdmi` is accepted for either connector name -- on a board that has only
/// one output, it is the output. The indexed name is still the fallback when
/// the tree cannot be read, because a wrong device name that ALSA then refuses
/// degrades audio while video keeps playing, which is the same outcome as
/// returning nothing but leaves the attempted name in the status document.
fn alsa_device_in(sound_root: &Path, name: &str) -> String {
    let preferred = if name == "HDMI-A-1" {
        "vc4hdmi0"
    } else {
        "vc4hdmi1"
    };
    let mut single = false;
    if let Ok(entries) = fs::read_dir(sound_root) {
        let mut ids = Vec::new();
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            if !file_name.starts_with("card") {
                continue;
            }
            if let Some(id) = read_line(&entry.path().join("id")) {
                ids.push(id);
            }
        }
        if ids.iter().any(|id| id == preferred) {
            return format!("plughw:CARD={preferred},DEV=0");
        }
        single = ids.iter().any(|id| id == "vc4hdmi");
    }
    if single {
        return "plughw:CARD=vc4hdmi,DEV=0".to_owned();
    }
    format!("plughw:CARD={preferred},DEV=0")
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

    /// A disposable set of roots standing in for `/sys/class/drm`, `/dev/dri`,
    /// and `/sys/class/sound`, removed when the test ends however it ends.
    struct Tree {
        root: PathBuf,
    }

    impl Tree {
        fn new(label: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "omt-connector-{label}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&root);
            let tree = Self { root };
            fs::create_dir_all(tree.sysfs()).unwrap_or_else(|error| panic!("{error}"));
            fs::create_dir_all(tree.devices()).unwrap_or_else(|error| panic!("{error}"));
            fs::create_dir_all(tree.sound()).unwrap_or_else(|error| panic!("{error}"));
            tree
        }

        fn sysfs(&self) -> PathBuf {
            self.root.join("sys")
        }

        fn devices(&self) -> PathBuf {
            self.root.join("dev")
        }

        fn sound(&self) -> PathBuf {
            self.root.join("sound")
        }

        /// Registers one ALSA card with the given id, as the kernel does.
        fn sound_card(&self, card: &str, id: &str) -> &Self {
            let entry = self.sound().join(card);
            fs::create_dir_all(&entry).unwrap_or_else(|error| panic!("{error}"));
            fs::write(entry.join("id"), id).unwrap_or_else(|error| panic!("{error}"));
            self
        }

        /// Adds one `cardN-NAME` entry. `status`/`connector_id` of `None` means
        /// the attribute is absent, which is how an unreadable card reads.
        fn card(&self, card: &str, name: &str, status: Option<&str>, id: Option<&str>) -> &Self {
            let entry = self.sysfs().join(format!("{card}-{name}"));
            fs::create_dir_all(&entry).unwrap_or_else(|error| panic!("{error}"));
            if let Some(status) = status {
                fs::write(entry.join("status"), status).unwrap_or_else(|error| panic!("{error}"));
            }
            if let Some(id) = id {
                fs::write(entry.join("connector_id"), id).unwrap_or_else(|error| panic!("{error}"));
            }
            self
        }

        fn device(&self, card: &str) -> &Self {
            fs::write(self.devices().join(card), b"").unwrap_or_else(|error| panic!("{error}"));
            self
        }

        fn find(&self, name: &str) -> Option<Connector> {
            named_in(&self.sysfs(), &self.devices(), &self.sound(), name)
        }

        fn alsa(&self, name: &str) -> String {
            alsa_device_in(&self.sound(), name)
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn selects_the_lowest_numbered_connected_card() {
        let tree = Tree::new("order");
        tree.card("card0", "HDMI-A-1", Some("connected"), Some("32"))
            .card("card1", "HDMI-A-1", Some("connected"), Some("48"))
            .device("card0")
            .device("card1");
        let selected = tree
            .find("HDMI-A-1")
            .unwrap_or_else(|| panic!("no connector"));
        assert_eq!(selected.id, 32);
        assert_eq!(selected.card_path, tree.devices().join("card0"));
        assert_eq!(selected.alsa_device, "plughw:CARD=vc4hdmi0,DEV=0");
    }

    /// The defect this loop was written to avoid: one card whose `status`
    /// cannot be read must disqualify that card, not the whole search. With an
    /// early return here the attached display on `card1` reads as absent and
    /// playback parks on `waiting-for-hdmi` indefinitely.
    #[test]
    fn an_unreadable_card_does_not_hide_a_later_connected_one() {
        let tree = Tree::new("unreadable");
        tree.card("card0", "HDMI-A-1", None, Some("32"))
            .card("card1", "HDMI-A-1", Some("connected"), Some("48"))
            .device("card0")
            .device("card1");
        assert_eq!(
            tree.find("HDMI-A-1").map(|connector| connector.id),
            Some(48)
        );
    }

    #[test]
    fn a_disconnected_or_half_populated_card_is_skipped() {
        let tree = Tree::new("skip");
        tree.card("card0", "HDMI-A-2", Some("disconnected"), Some("32"))
            // Present but with no connector id at all.
            .card("card1", "HDMI-A-2", Some("connected"), None)
            // A zero id is the kernel's "not yet assigned", not a connector.
            .card("card2", "HDMI-A-2", Some("connected"), Some("0"))
            // Announced in sysfs, but the render node never appeared.
            .card("card3", "HDMI-A-2", Some("connected"), Some("64"))
            .card("card4", "HDMI-A-2", Some("connected"), Some("80"))
            .device("card0")
            .device("card1")
            .device("card2")
            .device("card4");
        let selected = tree
            .find("HDMI-A-2")
            .unwrap_or_else(|| panic!("no connector"));
        assert_eq!(selected.id, 80);
        assert_eq!(selected.alsa_device, "plughw:CARD=vc4hdmi1,DEV=0");
    }

    #[test]
    fn no_usable_card_reads_as_no_display() {
        let tree = Tree::new("none");
        tree.card("card0", "HDMI-A-1", Some("disconnected"), Some("32"))
            .device("card0");
        assert!(tree.find("HDMI-A-1").is_none());
        // A different connector's cards never satisfy this name.
        assert!(tree.find("HDMI-A-2").is_none());
    }

    /// `is_connected` re-reads sysfs on every hotplug poll, so it has to reject
    /// a card that kept its name but was renumbered underneath the binding.
    #[test]
    fn a_renumbered_card_is_no_longer_the_bound_connector() {
        let tree = Tree::new("renumber");
        tree.card("card0", "HDMI-A-1", Some("connected"), Some("32"))
            .device("card0");
        let selected = tree
            .find("HDMI-A-1")
            .unwrap_or_else(|| panic!("no connector"));
        assert!(selected.is_connected());
        tree.card("card0", "HDMI-A-1", Some("connected"), Some("48"));
        assert!(!selected.is_connected());
        tree.card("card0", "HDMI-A-1", Some("disconnected"), Some("32"));
        assert!(!selected.is_connected());
    }

    /// Pi 4 and Pi 5: one ALSA card per HDMI output.
    #[test]
    fn maps_each_output_to_its_own_audio_card() {
        let tree = Tree::new("audio-dual");
        tree.sound_card("card0", "vc4hdmi0")
            .sound_card("card1", "vc4hdmi1");
        assert_eq!(tree.alsa("HDMI-A-1"), "plughw:CARD=vc4hdmi0,DEV=0");
        assert_eq!(tree.alsa("HDMI-A-2"), "plughw:CARD=vc4hdmi1,DEV=0");
    }

    /// The Pi 3 and Zero 2 W have one HDMI and register a single, unindexed
    /// `vc4hdmi` card. Asking for `vc4hdmi0` there is a device ALSA cannot
    /// open, which is what made HDMI audio fail silently on those boards while
    /// video kept playing.
    #[test]
    fn a_single_hdmi_board_uses_its_unindexed_card() {
        let tree = Tree::new("audio-single");
        tree.sound_card("card0", "vc4hdmi");
        assert_eq!(tree.alsa("HDMI-A-1"), "plughw:CARD=vc4hdmi,DEV=0");
        // Never selected on such a board, but it must not resolve to an
        // indexed card that does not exist either.
        assert_eq!(tree.alsa("HDMI-A-2"), "plughw:CARD=vc4hdmi,DEV=0");
    }

    /// An indexed card is preferred over a lone `vc4hdmi` when both somehow
    /// appear, and unrelated cards are ignored rather than matched by position.
    #[test]
    fn unrelated_audio_cards_are_ignored() {
        let tree = Tree::new("audio-mixed");
        tree.sound_card("card0", "Headphones")
            .sound_card("card1", "vc4hdmi0")
            .sound_card("card2", "vc4hdmi1");
        assert_eq!(tree.alsa("HDMI-A-1"), "plughw:CARD=vc4hdmi0,DEV=0");
        assert_eq!(tree.alsa("HDMI-A-2"), "plughw:CARD=vc4hdmi1,DEV=0");
    }

    /// With no readable sound tree the indexed name is still reported, so the
    /// status document names the device that was attempted.
    #[test]
    fn an_unreadable_sound_tree_falls_back_to_the_indexed_card() {
        let tree = Tree::new("audio-empty");
        assert_eq!(tree.alsa("HDMI-A-1"), "plughw:CARD=vc4hdmi0,DEV=0");
        assert_eq!(
            alsa_device_in(Path::new("/nonexistent-omt-sound-root"), "HDMI-A-2"),
            "plughw:CARD=vc4hdmi1,DEV=0"
        );
    }

    #[test]
    fn an_unknown_connector_is_never_selected() {
        assert!(find("HDMI-A-9").is_none());
        assert!(find("../../etc").is_none());
    }
}
