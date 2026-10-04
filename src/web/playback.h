/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * What the dashboard knows about playback: the saved source, the discovered
 * sources (cached briefly), the receiver's status record, and the actions
 * that change them through control-omt.sh.
 */
#ifndef OMT_WEB_PLAYBACK_H
#define OMT_WEB_PLAYBACK_H

#include "common/base.h"
#include "common/proc.h"
#include "web/settings.h"
#include "web/state.h"

#define OMT_ACTION_TEXT 1024
#define OMT_MAX_CHOICES 256

typedef struct {
    bool ok;
    char message[OMT_ACTION_TEXT];
    char error[OMT_ACTION_TEXT];
} omt_action_result;

typedef struct {
    char source[OMT_TARGET_MAX_BYTES + 1];
    char direct_address[OMT_TARGET_MAX_BYTES + 1];
    char error[256];
} omt_source_configuration;

typedef struct {
    char state[32];
    char label[64];
    char detail[2100];
    char tone[16];
    char source[OMT_TARGET_MAX_BYTES + 1];
    char direct_address[OMT_TARGET_MAX_BYTES + 1];
} omt_playback_summary;

typedef struct {
    char board_label[256];
    char effective[256];
    char board_default[256];
    char error[256];
    char effective_description[512];
    char board_default_description[512];
    bool overridden;
    bool above_board_default;
} omt_video_limit;

typedef struct {
    char names[OMT_MAX_CHOICES][OMT_SOURCE_NAME_MAX_BYTES + 1];
    size_t count;
} omt_source_choices;

typedef struct {
    const omt_web_settings *settings;
    uint64_t cache_expires_ms;
    omt_source_choices cache;
} omt_playback;

void omt_playback_init(omt_playback *p, const omt_web_settings *s);
void omt_playback_configuration(const omt_playback *p, omt_source_configuration *out);
static inline bool omt_configuration_configured(const omt_source_configuration *c) {
    return c->source[0] && !c->error[0];
}
void omt_playback_sources(omt_playback *p, omt_source_choices *out);
void omt_playback_refresh(omt_playback *p);
void omt_playback_select(omt_playback *p, const char *selection, omt_action_result *out);
void omt_playback_save_direct(omt_playback *p, const char *address, omt_action_result *out);
void omt_playback_restart(omt_playback *p, omt_action_result *out);
void omt_playback_clear(omt_playback *p, omt_action_result *out);
void omt_playback_video_limit(const omt_playback *p, omt_video_limit *out);
void omt_playback_save_video_limit(omt_playback *p, const char *value, omt_action_result *out);
void omt_playback_summary_read(omt_playback *p, omt_playback_summary *out);

/* Parses `omt-receiver discover --json` output: deduplicated, sorted, and
 * only names the shared grammar accepts with name == target. */
void omt_parse_discovered(const char *output, size_t len, omt_source_choices *out);
/* Whether a status record is well-formed, about `expected`, and fresh. */
bool omt_status_record_valid(const char *json, size_t len, const char *expected,
                             uint64_t stale_seconds, char *state, size_t state_size, char *detail,
                             size_t detail_size);

/* Runs the controller (control-omt.sh ACTION) with the configured timeout. */
void omt_control(const omt_web_settings *s, const char *action, double timeout_s,
                 omt_proc_result *out);

#endif
