/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Prepare an already-flashed Alpine boot partition for its first headless
 * boot: the pinned headless overlay and a Wi-Fi configuration that carries a
 * derived PSK rather than the passphrase.
 */
#include <errno.h>
#include <string.h>

#include "crypto/crypto.h"
#include "deploy/core/deploy.h"
#include "deploy/core/https.h"

#define HEADLESS_URL                                                                               \
    "https://github.com/macmpi/alpine-linux-headless-bootstrap/raw/"                               \
    "c426178c078c79e691c30e9eb89a4456cdeb62b2/headless.apkovl.tar.gz"
#define HEADLESS_SHA512                                                                            \
    "86bd4402b10aba589d4d9423e6b89521a1ea0c222b1f050eb6ef1348e877358b"                             \
    "9419714dbfd27a96527e4310dce4380cc5790eedceba40935084b0e33c185f13"
#define MAX_HEADLESS_BYTES (1024u * 1024u)
#define DOWNLOAD_TIMEOUT_MS 30000u

/* As Rust's io::Error displays a missing path. */
static void not_found(dp_err *err) { dp_fail(err, "No such file or directory (os error 2)"); }

const char *dp_validate_sd_settings(const dp_sd_settings *s, dp_err *err) {
    const char *country = s->country ? s->country : "";
    if (strlen(country) != 2 || country[0] < 'A' || country[0] > 'Z' || country[1] < 'A' ||
        country[1] > 'Z') {
        return "Wi-Fi country must be two uppercase ASCII letters.";
    }
    dp_wifi_settings wifi = {s->wifi_ssid, s->wifi_password, false, true};
    const char *problem = dp_validate_wifi(&wifi);
    if (problem) return problem;

    const char *boot = s->boot_directory ? s->boot_directory : "";
    dp_path_kind kind = dp_path_lstat(boot);
    if (kind == DP_PATH_MISSING || kind == DP_PATH_ERROR) {
        not_found(err);
        return dp_err_text(err);
    }
    if (kind != DP_PATH_DIR) return "Boot partition path must be a real directory.";
    omt_buf path;
    omt_buf_init(&path, 8192);
    dp_path_join(&path, boot, ".alpine-release");
    kind = dp_path_lstat(omt_buf_cstr(&path));
    if (kind != DP_PATH_FILE) {
        omt_buf_free(&path);
        return "The selected directory is not an Alpine boot partition.";
    }
    omt_buf_clear(&path);
    dp_path_join(&path, boot, "config.txt");
    dp_path_kind config = dp_path_lstat(omt_buf_cstr(&path));
    omt_buf_clear(&path);
    dp_path_join(&path, boot, "boot");
    dp_path_kind boot_dir = dp_path_lstat(omt_buf_cstr(&path));
    omt_buf_free(&path);
    if (config == DP_PATH_MISSING || boot_dir == DP_PATH_MISSING) {
        not_found(err);
        return dp_err_text(err);
    }
    if (config != DP_PATH_FILE || boot_dir != DP_PATH_DIR) {
        return "The selected directory is not an Alpine Raspberry Pi boot partition.";
    }
    return NULL;
}

bool dp_wpa_supplicant_config(const dp_sd_settings *s, omt_buf *out, dp_err *err) {
    dp_secret psk;
    dp_secret_init(&psk);
    const char *problem = dp_derive_wpa_psk(s->wifi_ssid, &s->wifi_password, &psk);
    if (problem) {
        dp_fail(err, "%s", problem);
        return false;
    }
    omt_buf_printf(out, "country=%s\nnetwork={\n    key_mgmt=WPA-PSK\n    ssid=", s->country);
    dp_hex_encode(out, s->wifi_ssid, strlen(s->wifi_ssid));
    omt_buf_puts(out, "\n    psk=");
    omt_buf_append(out, psk.value.data, psk.value.len);
    omt_buf_puts(out, "\n}\n");
    dp_secret_clear(&psk);
    return !out->failed;
}

static bool checked_target(const char *directory, const char *name, omt_buf *out, dp_err *err) {
    dp_path_join(out, directory, name);
    dp_path_kind kind = dp_path_lstat(omt_buf_cstr(out));
    if (kind == DP_PATH_MISSING || kind == DP_PATH_FILE) return true;
    if (kind == DP_PATH_ERROR) {
        dp_fail_os(err, omt_buf_cstr(out));
    } else {
        dp_fail(err, "A destination path exists but is not a regular file.");
    }
    return false;
}

static bool download_headless(const dp_cancel *cancel, omt_buf *bytes, dp_err *err) {
    if (!dp_https_get(HEADLESS_URL, MAX_HEADLESS_BYTES, DOWNLOAD_TIMEOUT_MS, cancel, bytes, err)) {
        omt_buf detail;
        omt_buf_init(&detail, DP_ERR_LIMIT);
        omt_buf_puts(&detail, dp_err_text(err));
        dp_fail(err, "headless overlay download failed: %s", omt_buf_cstr(&detail));
        omt_buf_free(&detail);
        return false;
    }
    uint8_t digest[OMT_SHA512_LEN];
    omt_buf hex;
    omt_buf_init(&hex, 256);
    bool ok = omt_sha512(bytes->data, bytes->len, digest);
    if (ok) {
        dp_hex_encode(&hex, digest, sizeof(digest));
        ok = strcmp(omt_buf_cstr(&hex), HEADLESS_SHA512) == 0;
    }
    omt_buf_free(&hex);
    if (!ok) dp_fail(err, "headless overlay checksum did not match the pinned release");
    return ok;
}

bool dp_prepare_sd_card(const dp_sd_settings *s, const dp_cancel *cancel, const dp_progress *p,
                        dp_err *err) {
    const char *problem = dp_validate_sd_settings(s, err);
    if (problem) {
        if (problem != dp_err_text(err)) dp_fail(err, "%s", problem);
        return false;
    }
    if (dp_cancelled(cancel)) {
        dp_fail(err, "cancelled");
        return false;
    }
    omt_buf headless_target, wifi_target, headless, wifi;
    omt_buf_init(&headless_target, 8192);
    omt_buf_init(&wifi_target, 8192);
    omt_buf_init(&headless, MAX_HEADLESS_BYTES + 1);
    omt_buf_init(&wifi, 4096);
    bool ok = checked_target(s->boot_directory, DP_HEADLESS_FILE_NAME, &headless_target, err) &&
              checked_target(s->boot_directory, "wpa_supplicant.conf", &wifi_target, err);
    if (ok) {
        dp_report(p, "Downloading verified headless bootstrap " DP_HEADLESS_VERSION "...");
        ok = download_headless(cancel, &headless, err);
    }
    if (ok && dp_cancelled(cancel)) {
        dp_fail(err, "cancelled");
        ok = false;
    }
    ok = ok && dp_wpa_supplicant_config(s, &wifi, err) &&
         dp_write_file_sync(omt_buf_cstr(&headless_target), headless.data, headless.len, err) &&
         dp_write_file_sync(omt_buf_cstr(&wifi_target), wifi.data, wifi.len, err);
    if (ok) {
        dp_report(p, "Wrote headless.apkovl.tar.gz and wpa_supplicant.conf to the Alpine boot "
                     "partition.");
    }
    omt_buf_free(&headless_target);
    omt_buf_free(&wifi_target);
    omt_buf_free(&headless);
    omt_buf_free_secret(&wifi);
    return ok;
}
