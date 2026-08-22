// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//! Prepare an already-flashed Alpine boot partition for its first headless boot.

use crate::{Secret, WifiSettings, derive_wpa_psk, hex_encode, validate_wifi};
use sha2::{Digest, Sha512};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const HEADLESS_FILE_NAME: &str = "headless.apkovl.tar.gz";
pub const HEADLESS_VERSION: &str = "v1.9";
const HEADLESS_URL: &str = "https://github.com/macmpi/alpine-linux-headless-bootstrap/raw/\
c426178c078c79e691c30e9eb89a4456cdeb62b2/headless.apkovl.tar.gz";
const HEADLESS_SHA512: &str = "86bd4402b10aba589d4d9423e6b89521a1ea0c222b1f050eb6ef1348e877358b\
9419714dbfd27a96527e4310dce4380cc5790eedceba40935084b0e33c185f13";
const MAX_HEADLESS_BYTES: u64 = 1024 * 1024;

pub struct SdCardSettings {
    pub boot_directory: PathBuf,
    pub country: String,
    pub wifi_ssid: String,
    pub wifi_password: Secret,
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

/// Validate the local boot partition and the Wi-Fi settings before downloading.
pub fn validate_sd_card_settings(settings: &SdCardSettings) -> io::Result<()> {
    if settings.country.len() != 2
        || !settings
            .country
            .bytes()
            .all(|byte| byte.is_ascii_uppercase())
    {
        return Err(invalid(
            "Wi-Fi country must be two uppercase ASCII letters.",
        ));
    }
    validate_wifi(&WifiSettings {
        ssid: settings.wifi_ssid.clone(),
        password: Secret::new(settings.wifi_password.expose().to_owned())
            .map_err(|error| invalid(error.0))?,
        connect: false,
        preserve_existing_profiles: true,
    })
    .map_err(|error| invalid(error.0))?;

    let metadata = fs::symlink_metadata(&settings.boot_directory)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid("Boot partition path must be a real directory."));
    }
    let release = settings.boot_directory.join(".alpine-release");
    let release_metadata = fs::symlink_metadata(&release).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            invalid("The selected directory is not an Alpine boot partition.")
        } else {
            error
        }
    })?;
    if !release_metadata.is_file() || release_metadata.file_type().is_symlink() {
        return Err(invalid(
            "The selected directory is not an Alpine boot partition.",
        ));
    }
    let config = fs::symlink_metadata(settings.boot_directory.join("config.txt"))?;
    let boot = fs::symlink_metadata(settings.boot_directory.join("boot"))?;
    if !config.is_file()
        || config.file_type().is_symlink()
        || !boot.is_dir()
        || boot.file_type().is_symlink()
    {
        return Err(invalid(
            "The selected directory is not an Alpine Raspberry Pi boot partition.",
        ));
    }
    Ok(())
}

/// Produce a Linux-text configuration without writing the plaintext passphrase.
pub fn wpa_supplicant_config(settings: &SdCardSettings) -> io::Result<String> {
    let psk = derive_wpa_psk(&settings.wifi_ssid, &settings.wifi_password)
        .map_err(|error| invalid(error.0))?;
    Ok(format!(
        "country={}\nnetwork={{\n    key_mgmt=WPA-PSK\n    ssid={}\n    psk={}\n}}\n",
        settings.country,
        hex_encode(settings.wifi_ssid.as_bytes()),
        psk.expose()
    ))
}

fn checked_target(directory: &Path, name: &str) -> io::Result<PathBuf> {
    let target = directory.join(name);
    match fs::symlink_metadata(&target) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Err(invalid(
            "A destination path exists but is not a regular file.",
        )),
        Ok(_) => Ok(target),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(target),
        Err(error) => Err(error),
    }
}

fn write_file(target: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(target)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn download_headless() -> io::Result<Vec<u8>> {
    let native = rustls_native_certs::load_native_certs();
    if native.certs.is_empty() {
        return Err(io::Error::other(format!(
            "no native TLS root certificates were available: {:?}",
            native.errors
        )));
    }
    let roots = native
        .certs
        .iter()
        .map(|der| ureq::tls::Certificate::from_der(der.as_ref()).to_owned())
        .collect();
    let tls = ureq::tls::TlsConfig::builder()
        .provider(ureq::tls::TlsProvider::Rustls)
        .root_certs(ureq::tls::RootCerts::Specific(Arc::new(roots)))
        .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .build();
    let config = ureq::Agent::config_builder()
        .https_only(true)
        .max_redirects(5)
        .max_redirects_will_error(true)
        .timeout_global(Some(Duration::from_secs(30)))
        .tls_config(tls)
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let mut response = agent
        .get(HEADLESS_URL)
        .call()
        .map_err(|error| io::Error::other(format!("headless overlay download failed: {error}")))?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(MAX_HEADLESS_BYTES)
        .read_to_vec()
        .map_err(|error| io::Error::other(format!("headless overlay download failed: {error}")))?;
    let digest = hex_encode(&Sha512::digest(&bytes));
    if digest != HEADLESS_SHA512 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "headless overlay checksum did not match the pinned release",
        ));
    }
    Ok(bytes)
}

/// Download the pinned headless overlay and write it and Wi-Fi configuration.
pub fn prepare_sd_card(
    settings: &SdCardSettings,
    cancellation: &Arc<AtomicBool>,
    progress: &mut dyn FnMut(&str),
) -> io::Result<()> {
    validate_sd_card_settings(settings)?;
    if cancellation.load(Ordering::SeqCst) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
    }
    let headless_target = checked_target(&settings.boot_directory, HEADLESS_FILE_NAME)?;
    let wifi_target = checked_target(&settings.boot_directory, "wpa_supplicant.conf")?;
    progress(&format!(
        "Downloading verified headless bootstrap {HEADLESS_VERSION}..."
    ));
    let headless = download_headless()?;
    if cancellation.load(Ordering::SeqCst) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
    }
    let wifi = wpa_supplicant_config(settings)?;
    write_file(&headless_target, &headless)?;
    write_file(&wifi_target, wifi.as_bytes())?;
    progress("Wrote headless.apkovl.tar.gz and wpa_supplicant.conf to the Alpine boot partition.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(path: &Path) -> SdCardSettings {
        SdCardSettings {
            boot_directory: path.to_path_buf(),
            country: "US".into(),
            wifi_ssid: "Studio Wi-Fi".into(),
            wifi_password: Secret::new("correct horse battery staple".into())
                .unwrap_or_else(|error| panic!("{error}")),
        }
    }

    #[test]
    fn wifi_file_uses_linux_lines_and_derived_credentials() {
        let value = wpa_supplicant_config(&settings(Path::new("unused")))
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(value.starts_with("country=US\nnetwork={\n"));
        assert!(!value.contains('\r'));
        assert!(!value.contains("Studio Wi-Fi"));
        assert!(!value.contains("correct horse battery staple"));
        assert!(value.contains("    ssid=53747564696f2057692d4669\n"));
        let psk = value
            .lines()
            .find_map(|line| line.strip_prefix("    psk="))
            .unwrap_or_else(|| panic!("missing psk in {value}"));
        assert_eq!(psk.len(), 64);
        assert!(psk.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn validation_requires_an_alpine_boot_partition_and_country() {
        let root = std::env::temp_dir().join(format!("omt-sd-card-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap_or_else(|error| panic!("{error}"));
        let mut value = settings(&root);
        let error = match validate_sd_card_settings(&value) {
            Ok(()) => panic!("a directory without an Alpine marker passed validation"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("not an Alpine boot partition"), "{error}");
        fs::write(root.join(".alpine-release"), b"3.24.1\n")
            .unwrap_or_else(|error| panic!("{error}"));
        fs::write(root.join("config.txt"), b"[all]\n").unwrap_or_else(|error| panic!("{error}"));
        fs::create_dir(root.join("boot")).unwrap_or_else(|error| panic!("{error}"));
        assert!(validate_sd_card_settings(&value).is_ok());
        value.country = "us".into();
        assert!(validate_sd_card_settings(&value).is_err());
        fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
    }
}
