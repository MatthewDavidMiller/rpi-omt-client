/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Receiver policy that has no hardware behind it: the decode ceiling, the
 * playback status projection the Web frontend reads, and detail sanitizing.
 */
#ifndef OMT_RECEIVER_CORE_H
#define OMT_RECEIVER_CORE_H

#include <pthread.h>

#include "common/base.h"
#include "common/buf.h"
#include "common/err.h"

#define OMT_HEARTBEAT_MS 500u
#define OMT_DETAIL_LIMIT 2048u

/* --- Video ceiling ---------------------------------------------------- */

/* The decode ceiling a board may attempt: a frame is admitted when it fits
 * inside any one shape. This is policy, not safety -- the protocol still
 * refuses anything above 1920x1080@60, which is what sizes allocations. */
#define OMT_CEILING_MAX_SHAPES 4
typedef struct {
    int32_t width, height, fps;
} omt_shape;
typedef struct {
    omt_shape shapes[OMT_CEILING_MAX_SHAPES];
    size_t count;
} omt_video_ceiling;

/* Parses WIDTHxHEIGHT@FPS[,...]. On failure `err` holds the operator-facing
 * reason. */
OMT_NODISCARD bool omt_ceiling_parse(const char *text, omt_video_ceiling *out, omt_err *err);
/* Whether the format fits; when not, `err` names the stream and ceiling. */
bool omt_ceiling_admits(const omt_video_ceiling *c, int32_t width, int32_t height, double rate,
                        omt_err *err);
void omt_ceiling_describe(const omt_video_ceiling *c, omt_buf *out);

/* --- Playback status --------------------------------------------------- */

typedef enum {
    OMT_VIDEO_RUNNING,
    OMT_VIDEO_WAITING_FOR_DISCOVERY,
    OMT_VIDEO_WAITING_FOR_HDMI,
    OMT_VIDEO_RETRYING,
    OMT_VIDEO_UNSUPPORTED_FORMAT,
    OMT_VIDEO_STARTING,
    OMT_VIDEO_STOPPED,
} omt_video_state;
#define OMT_VIDEO_STATE_COUNT 7

typedef enum { OMT_AUDIO_STOPPED, OMT_AUDIO_RUNNING, OMT_AUDIO_FAILED } omt_audio_state;

const char *omt_video_state_name(omt_video_state state);
const char *omt_audio_state_name(omt_audio_state state);

typedef struct {
    char name[32];
    char drm_device[64];
    char alsa_device[96];
} omt_connector;

void omt_connector_none(omt_connector *c);
bool omt_connector_equal(const omt_connector *a, const omt_connector *b);

typedef struct {
    omt_video_state video;
    omt_audio_state audio;
    char video_detail[OMT_DETAIL_LIMIT + 1];
    char audio_detail[OMT_DETAIL_LIMIT + 1];
    omt_connector connector;
} omt_projection;

typedef struct {
    char path[4096];
    char *target;
    pthread_mutex_t lock;
    omt_projection current;
    omt_projection published;
    bool has_published;
    uint64_t published_at_ms;
    uint64_t sequence;
    bool directory_ready;
} omt_playback_status;

/* `target` is copied. Returns false only when memory runs out. */
OMT_NODISCARD bool omt_status_init(omt_playback_status *s, const char *path, const char *target);
void omt_status_destroy(omt_playback_status *s);
/* Each returns 1 when the document was rewritten, 0 when nothing changed
 * inside the heartbeat, and -1 with `err` set when the write failed. */
int omt_status_video(omt_playback_status *s, omt_video_state state, const char *detail,
                     const omt_connector *connector, omt_err *err);
int omt_status_audio(omt_playback_status *s, omt_audio_state state, const char *detail,
                     const omt_connector *connector, omt_err *err);
int omt_status_heartbeat(omt_playback_status *s, const omt_connector *connector, omt_err *err);
int omt_status_stopped(omt_playback_status *s, const char *detail, omt_err *err);

/* Drops control characters, keeps at most OMT_DETAIL_LIMIT bytes of whole
 * characters, and trims Unicode whitespace. Invalid UTF-8 is replaced first. */
void omt_sanitize_detail(const char *value, char out[OMT_DETAIL_LIMIT + 1]);
/* Status files under /run live on tmpfs and are not fsynced. */
bool omt_durable_status_parent(const char *parent);

#endif
