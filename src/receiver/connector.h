/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * HDMI connector selection through sysfs. A connector name resolves to a card
 * device, a DRM connector id, and the matching ALSA device, and the binding is
 * re-checked on every hotplug poll so a card renumber cannot make the receiver
 * drive the wrong display.
 */
#ifndef OMT_RECEIVER_CONNECTOR_H
#define OMT_RECEIVER_CONNECTOR_H

#include "common/base.h"
#include "receiver_core/core.h"

typedef struct {
    char name[16];
    char card_path[512];
    char sysfs_path[512];
    uint32_t id;
    char alsa_device[96];
} omt_hdmi;

/* Finds a connector by name, or the first supported one for "auto". */
bool omt_hdmi_find(const char *preference, omt_hdmi *out);
/* The selection over explicit roots, so it can be tested without a Pi. */
bool omt_hdmi_named_in(const char *sysfs_root, const char *device_root, const char *sound_root,
                       const char *name, omt_hdmi *out);
void omt_hdmi_alsa_device_in(const char *sound_root, const char *name, char out[96]);
bool omt_hdmi_is_connected(const omt_hdmi *c);
void omt_hdmi_describe(const omt_hdmi *c, omt_connector *out);

#endif
