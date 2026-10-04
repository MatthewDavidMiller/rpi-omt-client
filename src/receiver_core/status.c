/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The playback status document the Web frontend reads. The video and audio
 * threads both report through here, so every entry point takes the lock.
 * Unchanged state is rewritten at most once per heartbeat, which is what lets
 * the consumer treat a stale timestamp as a receiver that stopped publishing.
 */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#include "common/fsio.h"
#include "common/json.h"
#include "common/proc.h"
#include "common/timefmt.h"
#include "receiver_core/core.h"

const char *omt_video_state_name(omt_video_state state) {
    switch (state) {
    case OMT_VIDEO_RUNNING: return "running";
    case OMT_VIDEO_WAITING_FOR_DISCOVERY: return "waiting-for-discovery";
    case OMT_VIDEO_WAITING_FOR_HDMI: return "waiting-for-hdmi";
    case OMT_VIDEO_RETRYING: return "retrying";
    case OMT_VIDEO_UNSUPPORTED_FORMAT: return "unsupported-format";
    case OMT_VIDEO_STARTING: return "starting";
    case OMT_VIDEO_STOPPED: return "stopped";
    }
    return "stopped";
}

const char *omt_audio_state_name(omt_audio_state state) {
    switch (state) {
    case OMT_AUDIO_STOPPED: return "stopped";
    case OMT_AUDIO_RUNNING: return "running";
    case OMT_AUDIO_FAILED: return "failed";
    }
    return "stopped";
}

void omt_connector_none(omt_connector *c) {
    memset(c, 0, sizeof(*c));
    omt_strlcpy(c->name, "none", sizeof(c->name));
    omt_strlcpy(c->drm_device, "none", sizeof(c->drm_device));
    omt_strlcpy(c->alsa_device, "none", sizeof(c->alsa_device));
}

bool omt_connector_equal(const omt_connector *a, const omt_connector *b) {
    return strcmp(a->name, b->name) == 0 && strcmp(a->drm_device, b->drm_device) == 0 &&
           strcmp(a->alsa_device, b->alsa_device) == 0;
}

void omt_sanitize_detail(const char *value, char out[OMT_DETAIL_LIMIT + 1]) {
    omt_buf clean;
    omt_buf_init(&clean, strlen(value) * 3 + 8);
    omt_utf8_lossy(&clean, value, strlen(value));
    const char *s = omt_buf_cstr(&clean);
    size_t len = clean.len, i = 0, n = 0;
    while (i < len) {
        size_t start = i;
        uint32_t cp = omt_utf8_next(s, len, &i);
        bool control = cp < 0x20 || (cp >= 0x7F && cp <= 0x9F);
        size_t width = i - start;
        if (!control && n + width <= OMT_DETAIL_LIMIT) {
            memcpy(out + n, s + start, width);
            n += width;
        }
    }
    out[n] = 0;
    omt_span trimmed = omt_utf8_trim(out, n);
    memmove(out, trimmed.p, trimmed.len);
    out[trimmed.len] = 0;
    omt_buf_free(&clean);
}

bool omt_durable_status_parent(const char *parent) {
    /* Path::starts_with compares whole components. */
    return !(strncmp(parent, "/run", 4) == 0 && (parent[4] == 0 || parent[4] == '/'));
}

bool omt_status_init(omt_playback_status *s, const char *path, const char *target) {
    memset(s, 0, sizeof(*s));
    if (!omt_strlcpy(s->path, path, sizeof(s->path))) return false;
    s->target = strdup(target);
    if (!s->target) return false;
    if (pthread_mutex_init(&s->lock, NULL) != 0) {
        free(s->target);
        return false;
    }
    s->current.video = OMT_VIDEO_STOPPED;
    s->current.audio = OMT_AUDIO_STOPPED;
    omt_strlcpy(s->current.video_detail, "Playback stopped.", sizeof(s->current.video_detail));
    omt_connector_none(&s->current.connector);
    return true;
}

void omt_status_destroy(omt_playback_status *s) {
    pthread_mutex_destroy(&s->lock);
    free(s->target);
    s->target = NULL;
}

static bool same(const omt_projection *a, const omt_projection *b) {
    return a->video == b->video && a->audio == b->audio &&
           strcmp(a->video_detail, b->video_detail) == 0 &&
           strcmp(a->audio_detail, b->audio_detail) == 0 &&
           omt_connector_equal(&a->connector, &b->connector);
}

/* Replaces the status file through a private stage and a rename. The parent
 * is created on the first publish only; if it later disappears the stage's
 * open fails and the caller reports it, which is the honest signal. */
static bool replace(omt_playback_status *s, const omt_buf *doc, omt_err *err) {
    char parent[4096];
    const char *slash = strrchr(s->path, '/');
    if (!slash) {
        omt_strlcpy(parent, ".", sizeof(parent));
    } else if (slash == s->path) {
        omt_strlcpy(parent, "/", sizeof(parent));
    } else {
        size_t n = (size_t)(slash - s->path);
        memcpy(parent, s->path, n);
        parent[n] = 0;
    }
    if (!s->directory_ready && !omt_mkdir_all(parent, 0777, err)) return false;
    char stage[4096 + 64];
    if (!omt_snprintf(stage, sizeof(stage), "%s/.omt-status.%ld.%016llx", parent, (long)getpid(),
                      (unsigned long long)s->sequence)) {
        omt_err_set(err, "status path too long");
        return false;
    }
    int fd = open(stage, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW, 0600);
    if (fd < 0) {
        omt_err_os(err, errno);
        return false;
    }
    bool durable = omt_durable_status_parent(parent);
    const uint8_t *p = doc->data;
    size_t left = doc->len;
    while (left) {
        ssize_t n = write(fd, p, left);
        if (n < 0) {
            if (errno == EINTR) continue;
            omt_err_os(err, errno);
            close(fd);
            unlink(stage);
            return false;
        }
        p += n;
        left -= (size_t)n;
    }
    /* Per-boot status on tmpfs cannot survive a restart, so fsync there is a
     * syscall tax twice a second for nothing. */
    if (durable && fsync(fd) != 0) {
        omt_err_os(err, errno);
        close(fd);
        unlink(stage);
        return false;
    }
    close(fd);
    if (rename(stage, s->path) != 0) {
        omt_err_os(err, errno);
        unlink(stage);
        return false;
    }
    if (durable) {
        int dfd = open(parent, O_RDONLY | O_DIRECTORY | O_CLOEXEC);
        if (dfd >= 0) {
            (void)fsync(dfd);
            close(dfd);
        }
    }
    return true;
}

static int publish(omt_playback_status *s, bool force, omt_err *err) {
    uint64_t now = omt_now_ms();
    bool unchanged = s->has_published && same(&s->published, &s->current);
    if (!force && unchanged && now - s->published_at_ms < OMT_HEARTBEAT_MS) return 0;
    const omt_projection *c = &s->current;
    const char *state, *detail;
    if (c->video == OMT_VIDEO_RUNNING && c->audio == OMT_AUDIO_FAILED) {
        state = "degraded";
        detail =
            c->audio_detail[0] ? c->audio_detail : "Video is playing but audio is unavailable.";
    } else {
        state = omt_video_state_name(c->video);
        detail = c->video_detail;
    }
    int64_t seconds;
    uint32_t nanos;
    omt_wall_clock(&seconds, &nanos);
    char stamp[32];
    omt_format_rfc3339_millis(seconds, nanos / 1000000u, stamp);

    omt_buf doc;
    omt_buf_init(&doc, 64 * 1024);
    omt_buf_puts(&doc, "{\"schema\":1,\"state\":");
    omt_json_write_cstr(&doc, state);
    omt_buf_puts(&doc, ",\"video_state\":");
    omt_json_write_cstr(&doc, omt_video_state_name(c->video));
    omt_buf_puts(&doc, ",\"audio_state\":");
    omt_json_write_cstr(&doc, omt_audio_state_name(c->audio));
    omt_buf_puts(&doc, ",\"target\":");
    omt_json_write_cstr(&doc, s->target);
    omt_buf_puts(&doc, ",\"detail\":");
    omt_json_write_cstr(&doc, detail);
    omt_buf_puts(&doc, ",\"connector\":");
    omt_json_write_cstr(&doc, c->connector.name);
    omt_buf_puts(&doc, ",\"drm_device\":");
    omt_json_write_cstr(&doc, c->connector.drm_device);
    omt_buf_puts(&doc, ",\"alsa_device\":");
    omt_json_write_cstr(&doc, c->connector.alsa_device);
    omt_buf_puts(&doc, ",\"updated_at\":");
    omt_json_write_cstr(&doc, stamp);
    omt_buf_putc(&doc, '}');
    if (doc.failed) {
        omt_buf_free(&doc);
        omt_err_set(err, "status document too large");
        return -1;
    }
    bool ok = replace(s, &doc, err);
    omt_buf_free(&doc);
    if (!ok) return -1;
    s->directory_ready = true;
    s->sequence++;
    s->published = s->current;
    s->has_published = true;
    s->published_at_ms = omt_now_ms();
    return 1;
}

/* Folds one worker's detail and connector into the projection. Both workers
 * call in on every frame with a detail that is almost always the one they
 * last sent, so the unchanged case compares the raw text before sanitizing. */
static void update_detail(omt_projection *current, char *which, bool state_changed,
                          const char *detail, const omt_connector *connector) {
    if (state_changed || !omt_connector_equal(&current->connector, connector)) {
        omt_sanitize_detail(detail, which);
        current->connector = *connector;
    } else if (strcmp(which, detail) != 0) {
        omt_sanitize_detail(detail, which);
    }
}

int omt_status_video(omt_playback_status *s, omt_video_state state, const char *detail,
                     const omt_connector *connector, omt_err *err) {
    pthread_mutex_lock(&s->lock);
    bool changed = s->current.video != state;
    s->current.video = state;
    update_detail(&s->current, s->current.video_detail, changed, detail, connector);
    int r = publish(s, false, err);
    pthread_mutex_unlock(&s->lock);
    return r;
}

int omt_status_audio(omt_playback_status *s, omt_audio_state state, const char *detail,
                     const omt_connector *connector, omt_err *err) {
    pthread_mutex_lock(&s->lock);
    bool changed = s->current.audio != state;
    s->current.audio = state;
    update_detail(&s->current, s->current.audio_detail, changed, detail, connector);
    int r = publish(s, false, err);
    pthread_mutex_unlock(&s->lock);
    return r;
}

int omt_status_heartbeat(omt_playback_status *s, const omt_connector *connector, omt_err *err) {
    pthread_mutex_lock(&s->lock);
    s->current.connector = *connector;
    int r = publish(s, false, err);
    pthread_mutex_unlock(&s->lock);
    return r;
}

int omt_status_stopped(omt_playback_status *s, const char *detail, omt_err *err) {
    pthread_mutex_lock(&s->lock);
    s->current.video = OMT_VIDEO_STOPPED;
    s->current.audio = OMT_AUDIO_STOPPED;
    omt_sanitize_detail(detail, s->current.video_detail);
    s->current.audio_detail[0] = 0;
    omt_connector_none(&s->current.connector);
    int r = publish(s, true, err);
    pthread_mutex_unlock(&s->lock);
    return r;
}
