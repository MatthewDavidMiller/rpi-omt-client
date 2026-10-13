//! Prepare an already-flashed Alpine boot partition for its first headless
//! boot: the pinned headless overlay and a Wi-Fi configuration that carries
//! a derived PSK rather than the passphrase.

use omt_common::{Error, Result};

use crate::core::{Progress, SdSettings, WifiSettings, derive_wpa_psk, report, validate_wifi};
use crate::https;
use crate::sys::{self, Cancel, PathKind};

pub const HEADLESS_FILE_NAME: &str = "headless.apkovl.tar.gz";
pub const HEADLESS_VERSION: &str = "v1.9";
const HEADLESS_URL: &str = "https://github.com/macmpi/alpine-linux-headless-bootstrap/raw/\
     c426178c078c79e691c30e9eb89a4456cdeb62b2/headless.apkovl.tar.gz";
const HEADLESS_SHA512: &str = "86bd4402b10aba589d4d9423e6b89521a1ea0c222b1f050eb6ef1348e877358b\
     9419714dbfd27a96527e4310dce4380cc5790eedceba40935084b0e33c185f13";
const MAX_HEADLESS_BYTES: usize = 1024 * 1024;
const DOWNLOAD_TIMEOUT_MS: u64 = 30_000;

/// As io::Error displays a missing path.
const NOT_FOUND: &str = "No such file or directory (os error 2)";

pub fn validate_sd_settings(s: &SdSettings) -> Option<&'static str> {
    let c = s.country.as_bytes();
    if c.len() != 2 || !c.iter().all(u8::is_ascii_uppercase) {
        return Some("Wi-Fi country must be two uppercase ASCII letters.");
    }
    let wifi = WifiSettings {
        ssid: s.wifi_ssid.clone(),
        password: s.wifi_password.clone(),
        connect: false,
        preserve_existing_profiles: true,
    };
    if let Some(problem) = validate_wifi(&wifi) {
        return Some(problem);
    }
    let boot = s.boot_directory.as_str();
    match sys::path_lstat(boot) {
        PathKind::Missing | PathKind::Error => return Some(NOT_FOUND),
        PathKind::Dir => {}
        _ => return Some("Boot partition path must be a real directory."),
    }
    if sys::path_lstat(sys::path_join(boot, ".alpine-release")) != PathKind::File {
        return Some("The selected directory is not an Alpine boot partition.");
    }
    let config = sys::path_lstat(sys::path_join(boot, "config.txt"));
    let boot_dir = sys::path_lstat(sys::path_join(boot, "boot"));
    if config == PathKind::Missing || boot_dir == PathKind::Missing {
        return Some(NOT_FOUND);
    }
    if config != PathKind::File || boot_dir != PathKind::Dir {
        return Some("The selected directory is not an Alpine Raspberry Pi boot partition.");
    }
    None
}

pub fn wpa_supplicant_config(s: &SdSettings) -> Result<String> {
    let psk = derive_wpa_psk(&s.wifi_ssid, &s.wifi_password).map_err(Error::new)?;
    Ok(format!(
        "country={}\nnetwork={{\n    key_mgmt=WPA-PSK\n    ssid={}\n    psk={}\n}}\n",
        s.country,
        omt_common::buf::hex(s.wifi_ssid.as_bytes()),
        psk.text()
    ))
}

fn checked_target(directory: &str, name: &str) -> Result<String> {
    let path = sys::path_join(directory, name);
    match sys::path_lstat(&path) {
        PathKind::Missing | PathKind::File => Ok(path),
        PathKind::Error => match std::fs::symlink_metadata(&path) {
            Err(e) => Err(sys::os_error(&path, &e)),
            Ok(_) => Err(Error::new(path)),
        },
        _ => Err(Error::new("A destination path exists but is not a regular file.")),
    }
}

fn download_headless(cancel: Option<&Cancel>) -> Result<Vec<u8>> {
    let bytes = https::get(HEADLESS_URL, MAX_HEADLESS_BYTES, DOWNLOAD_TIMEOUT_MS, cancel)
        .map_err(|e| Error::new(format!("headless overlay download failed: {e}")))?;
    let ok = omt_crypto::sha512(&bytes).is_ok_and(|d| omt_common::buf::hex(&d) == HEADLESS_SHA512);
    if !ok {
        return Err(Error::new("headless overlay checksum did not match the pinned release"));
    }
    Ok(bytes)
}

pub fn prepare_sd_card(
    s: &SdSettings,
    cancel: Option<&Cancel>,
    p: Option<Progress<'_>>,
) -> Result<()> {
    if let Some(problem) = validate_sd_settings(s) {
        return Err(Error::new(problem));
    }
    let cancelled = || Error::new("cancelled");
    if sys::cancelled(cancel) {
        return Err(cancelled());
    }
    let headless_target = checked_target(&s.boot_directory, HEADLESS_FILE_NAME)?;
    let wifi_target = checked_target(&s.boot_directory, "wpa_supplicant.conf")?;
    report(p, &format!("Downloading verified headless bootstrap {HEADLESS_VERSION}..."));
    let headless = download_headless(cancel)?;
    if sys::cancelled(cancel) {
        return Err(cancelled());
    }
    let mut wifi = wpa_supplicant_config(s)?;
    let written = sys::write_file_sync(&headless_target, &headless)
        .and_then(|()| sys::write_file_sync(&wifi_target, wifi.as_bytes()));
    omt_common::buf::wipe_string(&mut wifi);
    written?;
    report(p, "Wrote headless.apkovl.tar.gz and wpa_supplicant.conf to the Alpine boot partition.");
    Ok(())
}
