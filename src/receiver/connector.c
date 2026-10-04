/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "receiver/connector.h"

#include <dirent.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#include "common/fsio.h"
#include "common/json.h"

#define SYSFS_ROOT "/sys/class/drm"
#define DEVICE_ROOT "/dev/dri"
#define SOUND_ROOT "/sys/class/sound"

/* Connector names the appliance supports, in auto-selection order. */
static const char *const SUPPORTED[] = {"HDMI-A-1", "HDMI-A-2"};

/* Reads a small sysfs attribute, trimmed. */
static bool read_line(const char *dir, const char *file, char *out, size_t size) {
    char path[1024];
    if (!omt_snprintf(path, sizeof(path), "%s/%s", dir, file)) return false;
    omt_buf b;
    omt_err err;
    if (omt_read_bounded(path, 4096, &b, &err) != OMT_READ_OK) return false;
    bool ok = omt_utf8_valid(omt_buf_cstr(&b), b.len);
    if (ok) {
        omt_span t = omt_utf8_trim(omt_buf_cstr(&b), b.len);
        ok = t.len < size;
        if (ok) {
            memcpy(out, t.p, t.len);
            out[t.len] = 0;
        }
    }
    omt_buf_free(&b);
    return ok;
}

static int compare_names(const void *a, const void *b) {
    return strcmp(*(const char *const *)a, *(const char *const *)b);
}

bool omt_hdmi_named_in(const char *sysfs_root, const char *device_root, const char *sound_root,
                       const char *name, omt_hdmi *out) {
    char suffix[32];
    if (!omt_snprintf(suffix, sizeof(suffix), "-%s", name)) return false;
    size_t suffix_len = strlen(suffix);
    DIR *dir = opendir(sysfs_root);
    if (!dir) return false;
    /* Several cards can expose the same connector name; the lowest-numbered
     * one that is actually connected wins, which keeps selection stable. */
    char *candidates[64];
    size_t count = 0;
    struct dirent *entry;
    while ((entry = readdir(dir)) && count < OMT_ARRAY_LEN(candidates)) {
        size_t n = strlen(entry->d_name);
        if (n > suffix_len && strncmp(entry->d_name, "card", 4) == 0 &&
            strcmp(entry->d_name + n - suffix_len, suffix) == 0) {
            candidates[count] = strdup(entry->d_name);
            if (candidates[count]) count++;
        }
    }
    closedir(dir);
    qsort(candidates, count, sizeof(char *), compare_names);
    bool found = false;
    /* Every rejection disqualifies that card only; the next candidate may
     * still be the display the operator plugged in. */
    for (size_t i = 0; i < count && !found; i++) {
        char sysfs_path[512], card_path[512], status[64], id_text[32];
        if (!omt_snprintf(sysfs_path, sizeof(sysfs_path), "%s/%s", sysfs_root, candidates[i]))
            continue;
        if (!read_line(sysfs_path, "status", status, sizeof(status)) ||
            strcmp(status, "connected") != 0)
            continue;
        uint64_t id;
        if (!read_line(sysfs_path, "connector_id", id_text, sizeof(id_text)) ||
            !omt_parse_u64(id_text, strlen(id_text), UINT32_MAX, &id) || id == 0)
            continue;
        size_t card_len = strlen(candidates[i]) - suffix_len;
        if (!omt_snprintf(card_path, sizeof(card_path), "%s/%.*s", device_root, (int)card_len,
                          candidates[i]))
            continue;
        if (access(card_path, F_OK) != 0) continue;
        memset(out, 0, sizeof(*out));
        omt_strlcpy(out->name, name, sizeof(out->name));
        omt_strlcpy(out->card_path, card_path, sizeof(out->card_path));
        omt_strlcpy(out->sysfs_path, sysfs_path, sizeof(out->sysfs_path));
        out->id = (uint32_t)id;
        omt_hdmi_alsa_device_in(sound_root, name, out->alsa_device);
        found = true;
    }
    for (size_t i = 0; i < count; i++) free(candidates[i]);
    return found;
}

/* Pi 4 and Pi 5 register one ALSA card per output (vc4hdmi0, vc4hdmi1); Pi 3
 * and Zero 2 W register a single unindexed vc4hdmi, which on a one-output
 * board is the output. When the tree cannot be read the indexed name is still
 * reported, so the status names the device that was attempted. */
void omt_hdmi_alsa_device_in(const char *sound_root, const char *name, char out[96]) {
    const char *preferred = strcmp(name, "HDMI-A-1") == 0 ? "vc4hdmi0" : "vc4hdmi1";
    bool single = false, has_preferred = false;
    DIR *dir = opendir(sound_root);
    if (dir) {
        struct dirent *entry;
        while ((entry = readdir(dir))) {
            if (strncmp(entry->d_name, "card", 4) != 0) continue;
            char path[1024], id[64];
            if (!omt_snprintf(path, sizeof(path), "%s/%s", sound_root, entry->d_name)) continue;
            if (!read_line(path, "id", id, sizeof(id))) continue;
            if (strcmp(id, preferred) == 0) has_preferred = true;
            if (strcmp(id, "vc4hdmi") == 0) single = true;
        }
        closedir(dir);
    }
    if (!has_preferred && single) {
        omt_strlcpy(out, "plughw:CARD=vc4hdmi,DEV=0", 96);
        return;
    }
    snprintf(out, 96, "plughw:CARD=%s,DEV=0", preferred);
}

bool omt_hdmi_find(const char *preference, omt_hdmi *out) {
    if (strcmp(preference, "auto") == 0) {
        for (size_t i = 0; i < OMT_ARRAY_LEN(SUPPORTED); i++)
            if (omt_hdmi_named_in(SYSFS_ROOT, DEVICE_ROOT, SOUND_ROOT, SUPPORTED[i], out))
                return true;
        return false;
    }
    for (size_t i = 0; i < OMT_ARRAY_LEN(SUPPORTED); i++)
        if (strcmp(preference, SUPPORTED[i]) == 0)
            return omt_hdmi_named_in(SYSFS_ROOT, DEVICE_ROOT, SOUND_ROOT, preference, out);
    return false;
}

bool omt_hdmi_is_connected(const omt_hdmi *c) {
    char status[64], id[32], expected[16];
    snprintf(expected, sizeof(expected), "%u", c->id);
    return read_line(c->sysfs_path, "status", status, sizeof(status)) &&
           strcmp(status, "connected") == 0 &&
           read_line(c->sysfs_path, "connector_id", id, sizeof(id)) && strcmp(id, expected) == 0;
}

void omt_hdmi_describe(const omt_hdmi *c, omt_connector *out) {
    memset(out, 0, sizeof(*out));
    omt_strlcpy(out->name, c->name, sizeof(out->name));
    omt_strlcpy(out->drm_device, c->card_path, sizeof(out->drm_device));
    omt_strlcpy(out->alsa_device, c->alsa_device, sizeof(out->alsa_device));
}
