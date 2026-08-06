// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#ifndef OMT_RECEIVER_PLAYBACK_STATUS_H
#define OMT_RECEIVER_PLAYBACK_STATUS_H

#include "native_types.h"

#include <stddef.h>

typedef struct omt_playback_status omt_playback_status;

omt_playback_status *omt_playback_status_create(const char *path, const char *target);
void omt_playback_status_destroy(omt_playback_status *status);
void omt_status_video_starting(omt_playback_status *, const char *, const omt_connector *);
void omt_status_waiting_for_discovery(omt_playback_status *, const char *, const omt_connector *);
void omt_status_waiting_for_hdmi(omt_playback_status *, const char *, const omt_connector *);
void omt_status_video_retrying(omt_playback_status *, const char *, const omt_connector *);
void omt_status_unsupported_format(omt_playback_status *, const char *, const omt_connector *);
void omt_status_video_running(omt_playback_status *, const char *, const omt_connector *);
void omt_status_audio_running(omt_playback_status *, const char *, const omt_connector *);
void omt_status_audio_failed(omt_playback_status *, const char *, const omt_connector *);
void omt_status_audio_stopped(omt_playback_status *, const omt_connector *);
void omt_status_heartbeat(omt_playback_status *, const omt_connector *);
void omt_status_stopped(omt_playback_status *, const char *, const omt_connector *);
size_t omt_sanitize_status_detail(char *output, size_t capacity, const char *value);

#endif
