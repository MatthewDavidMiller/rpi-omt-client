/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The OMT SDK's network settings file (settings.xml) and its one field the
 * Web frontend edits: the central Discovery Server.
 */
#ifndef OMT_WEB_NETWORK_H
#define OMT_WEB_NETWORK_H

#include "common/base.h"
#include "common/buf.h"
#include "common/err.h"

typedef struct {
    char discovery_server[600];
    char error[256];
} omt_network_configuration;

/* Normalizes operator input to omt://host:port, or "" for an empty field. */
OMT_NODISCARD bool omt_normalize_server(const char *value, char out[600], omt_err *err);
/* Strict read of the configured server from a settings document. */
OMT_NODISCARD bool omt_parse_server(const uint8_t *xml, size_t len, char out[600], omt_err *err);
void omt_read_network_configuration(const char *path, omt_network_configuration *out);
/* Rewrites only the DiscoveryServer value, keeping every other byte of the
 * operator's file; NULL when the document already says the same. */
OMT_NODISCARD bool omt_update_settings_xml(const uint8_t *xml, size_t len, const char *normalized,
                                           omt_buf *out, bool *changed, omt_err *err);
/* 1 when the file changed, 0 when it already matched, -1 with `err`. */
int omt_save_network_configuration(const char *path, const char *value, omt_err *err);

#endif
