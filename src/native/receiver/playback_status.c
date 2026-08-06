// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#define _POSIX_C_SOURCE 200809L
#include "playback_status.h"

#include "json_text.h"

#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#define DETAIL_LIMIT 2048u
#define STATUS_DOCUMENT_CAPACITY 16384u

struct omt_playback_status {
    pthread_mutex_t mutex;
    char path[OMT_PATH_CAPACITY];
    char target[OMT_ENDPOINT_HOST_CAPACITY + 16u];
    char video_state[32];
    char audio_state[32];
    char video_detail[DETAIL_LIMIT + 1u];
    char audio_detail[DETAIL_LIMIT + 1u];
    char published_state[32];
    char published_video[32];
    char published_audio[32];
    char published_detail[DETAIL_LIMIT + 1u];
    char published_connector[OMT_CONNECTOR_NAME_CAPACITY];
    struct timespec published_at;
};

static atomic_uint_fast64_t stage_counter;

static bool write_all(int fd, const char *value, size_t length)
{
    size_t offset = 0u;
    while (offset < length) {
        ssize_t count = write(fd, value + offset, length - offset);
        if (count > 0) {
            offset += (size_t)count;
        } else if (count < 0 && errno == EINTR) {
            continue;
        } else {
            return false;
        }
    }
    return true;
}

static void parent_directory(const char *path, char *output, size_t capacity)
{
    const char *slash = strrchr(path, '/');
    if (slash == NULL) {
        (void)omt_copy_string(output, capacity, ".");
        return;
    }
    size_t length = slash == path ? 1u : (size_t)(slash - path);
    if (length >= capacity) {
        output[0] = '\0';
        return;
    }
    memcpy(output, path, length);
    output[length] = '\0';
}

/* Create every missing component, not just the last one: a single mkdir fails
   with ENOENT when the grandparent is absent and the status would then never be
   written, leaving the dashboard pinned to a stale document with no diagnostic. */
static bool create_directories(const char *path)
{
    char buffer[OMT_PATH_CAPACITY];
    struct stat info;
    if (!omt_copy_string(buffer, sizeof(buffer), path)) {
        return false;
    }
    for (char *cursor = buffer + 1; *cursor != '\0'; ++cursor) {
        if (*cursor != '/') {
            continue;
        }
        *cursor = '\0';
        if (mkdir(buffer, 0700) != 0 && errno != EEXIST) {
            return false;
        }
        *cursor = '/';
    }
    if (mkdir(buffer, 0700) == 0) {
        return true;
    }
    return errno == EEXIST && stat(buffer, &info) == 0 && S_ISDIR(info.st_mode);
}

static void atomic_replace(const char *path, const char *content, size_t length)
{
    char directory[OMT_PATH_CAPACITY];
    char stage[OMT_PATH_CAPACITY];
    parent_directory(path, directory, sizeof(directory));
    if (directory[0] == '\0' || !create_directories(directory)) {
        return;
    }
    int count = snprintf(stage, sizeof(stage), "%s/.omt-status.%ld.%016llx", directory,
                         (long)getpid(), (unsigned long long)atomic_fetch_add(&stage_counter, 1u));
    if (count < 0 || (size_t)count >= sizeof(stage)) {
        return;
    }
    int fd = open(stage, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW, 0600);
    if (fd < 0) {
        return;
    }
    bool committed = write_all(fd, content, length) && fsync(fd) == 0;
    if (close(fd) != 0) {
        committed = false;
    }
    if (committed && rename(stage, path) == 0) {
        int directory_fd = open(directory, O_RDONLY | O_DIRECTORY | O_CLOEXEC);
        if (directory_fd >= 0) {
            (void)fsync(directory_fd);
            (void)close(directory_fd);
        }
        return;
    }
    (void)unlink(stage);
}

size_t omt_sanitize_status_detail(char *output, size_t capacity, const char *value)
{
    size_t input = 0u;
    size_t used = 0u;
    if (output == NULL || capacity == 0u || value == NULL) {
        return 0u;
    }
    while (value[input] != '\0' && used < DETAIL_LIMIT && used + 1u < capacity) {
        unsigned char first = (unsigned char)value[input];
        size_t scalar_length = 1u;
        uint32_t scalar = first;
        uint32_t minimum = 0u;
        if (first < 0x80u) {
        } else if ((first & 0xe0u) == 0xc0u) {
            scalar_length = 2u; scalar = first & 0x1fu; minimum = 0x80u;
        } else if ((first & 0xf0u) == 0xe0u) {
            scalar_length = 3u; scalar = first & 0x0fu; minimum = 0x800u;
        } else if ((first & 0xf8u) == 0xf0u) {
            scalar_length = 4u; scalar = first & 0x07u; minimum = 0x10000u;
        } else {
            input++; continue;
        }
        bool valid = true;
        for (size_t offset = 1u; offset < scalar_length; ++offset) {
            unsigned char next = (unsigned char)value[input + offset];
            if (next == 0u || (next & 0xc0u) != 0x80u) {
                valid = false; break;
            }
            scalar = (scalar << 6u) | (next & 0x3fu);
        }
        valid = valid && (scalar_length == 1u || scalar >= minimum) && scalar <= 0x10ffffu &&
                !(scalar >= 0xd800u && scalar <= 0xdfffu);
        if (!valid) {
            input++; continue;
        }
        if (used + scalar_length > DETAIL_LIMIT || used + scalar_length >= capacity) {
            break;
        }
        if (scalar >= 0x20u && scalar != 0x7fu) {
            memcpy(output + used, value + input, scalar_length);
            used += scalar_length;
        }
        input += scalar_length;
    }
    output[used] = '\0';
    size_t begin = 0u;
    while (output[begin] == ' ' || output[begin] == '\t' ||
           output[begin] == '\r' || output[begin] == '\n') {
        begin++;
    }
    while (used > begin && (output[used - 1u] == ' ' || output[used - 1u] == '\t' ||
                            output[used - 1u] == '\r' || output[used - 1u] == '\n')) {
        used--;
    }
    if (begin != 0u && used > begin) {
        memmove(output, output + begin, used - begin);
    }
    used = used > begin ? used - begin : 0u;
    output[used] = '\0';
    return used;
}

static void timestamp(char output[40])
{
    struct timespec now = {0, 0};
    struct tm utc = {0};
    (void)clock_gettime(CLOCK_REALTIME, &now);
    (void)gmtime_r(&now.tv_sec, &utc);
    size_t used = strftime(output, 40u, "%Y-%m-%dT%H:%M:%S", &utc);
    unsigned int milliseconds = (unsigned int)(now.tv_nsec / 1000000L);
    if (used == 0u || used >= 34u) {
        output[0] = '\0';
        return;
    }
    (void)snprintf(output + used, 40u - used, ".%03uZ", milliseconds);
}

static void publish_locked(omt_playback_status *status, const omt_connector *connector, bool force)
{
    const char *state = status->video_state;
    const char *detail = status->video_detail;
    const char *connector_name = connector == NULL ? "none" : connector->name;
    if (strcmp(status->video_state, "running") == 0 && strcmp(status->audio_state, "failed") == 0) {
        state = "degraded";
        detail = status->audio_detail[0] == '\0'
            ? "Video is playing but audio is unavailable." : status->audio_detail;
    }
    struct timespec now = {0, 0};
    (void)clock_gettime(CLOCK_MONOTONIC, &now);
    bool changed = strcmp(state, status->published_state) != 0 ||
        strcmp(status->video_state, status->published_video) != 0 ||
        strcmp(status->audio_state, status->published_audio) != 0 ||
        strcmp(detail, status->published_detail) != 0 ||
        strcmp(connector_name, status->published_connector) != 0;
    int64_t elapsed_ms = ((int64_t)now.tv_sec - (int64_t)status->published_at.tv_sec) * 1000 +
        ((int64_t)now.tv_nsec - (int64_t)status->published_at.tv_nsec) / 1000000;
    if (!force && !changed && status->published_at.tv_sec != 0 && elapsed_ms < 500) {
        return;
    }
    (void)omt_copy_string(status->published_state, sizeof(status->published_state), state);
    (void)omt_copy_string(status->published_video, sizeof(status->published_video), status->video_state);
    (void)omt_copy_string(status->published_audio, sizeof(status->published_audio), status->audio_state);
    (void)omt_copy_string(status->published_detail, sizeof(status->published_detail), detail);
    (void)omt_copy_string(status->published_connector, sizeof(status->published_connector), connector_name);
    status->published_at = now;

    char document[STATUS_DOCUMENT_CAPACITY];
    char updated_at[40];
    timestamp(updated_at);
    omt_text_buffer text;
    omt_text_init(&text, document, sizeof(document));
#define FIELD(prefix, value) do { (void)omt_text_append(&text, prefix); (void)omt_json_append_string(&text, value); } while (0)
    (void)omt_text_append(&text, "{\"schema\":1");
    FIELD(",\"state\":", state);
    FIELD(",\"video_state\":", status->video_state);
    FIELD(",\"audio_state\":", status->audio_state);
    FIELD(",\"target\":", status->target);
    FIELD(",\"detail\":", detail);
    FIELD(",\"connector\":", connector_name);
    FIELD(",\"drm_device\":", connector == NULL ? "none" : connector->device_path);
    FIELD(",\"alsa_device\":", connector == NULL ? "none" : connector->alsa_device);
    FIELD(",\"updated_at\":", updated_at);
    (void)omt_text_append_char(&text, '}');
#undef FIELD
    if (!text.failed) {
        atomic_replace(status->path, document, text.length);
    }
}

omt_playback_status *omt_playback_status_create(const char *path, const char *target)
{
    omt_playback_status *status = calloc(1u, sizeof(*status));
    if (status == NULL || !omt_copy_string(status->path, sizeof(status->path), path) ||
        !omt_copy_string(status->target, sizeof(status->target), target) ||
        pthread_mutex_init(&status->mutex, NULL) != 0) {
        free(status);
        return NULL;
    }
    (void)omt_copy_string(status->video_state, sizeof(status->video_state), "stopped");
    (void)omt_copy_string(status->audio_state, sizeof(status->audio_state), "stopped");
    (void)omt_copy_string(status->video_detail, sizeof(status->video_detail), "Playback stopped.");
    return status;
}

void omt_playback_status_destroy(omt_playback_status *status)
{
    if (status != NULL) {
        (void)pthread_mutex_destroy(&status->mutex);
        free(status);
    }
}

static void set_video(omt_playback_status *status, const char *state, const char *detail,
                      const omt_connector *connector)
{
    (void)pthread_mutex_lock(&status->mutex);
    (void)omt_copy_string(status->video_state, sizeof(status->video_state), state);
    (void)omt_sanitize_status_detail(status->video_detail, sizeof(status->video_detail), detail);
    publish_locked(status, connector, false);
    (void)pthread_mutex_unlock(&status->mutex);
}

static void set_audio(omt_playback_status *status, const char *state, const char *detail,
                      const omt_connector *connector)
{
    (void)pthread_mutex_lock(&status->mutex);
    (void)omt_copy_string(status->audio_state, sizeof(status->audio_state), state);
    (void)omt_sanitize_status_detail(status->audio_detail, sizeof(status->audio_detail), detail);
    publish_locked(status, connector, false);
    (void)pthread_mutex_unlock(&status->mutex);
}

#define VIDEO_FN(name, state) \
    void name(omt_playback_status *s, const char *d, const omt_connector *c) { set_video(s, state, d, c); }
VIDEO_FN(omt_status_video_starting, "starting")
VIDEO_FN(omt_status_waiting_for_discovery, "waiting-for-discovery")
VIDEO_FN(omt_status_waiting_for_hdmi, "waiting-for-hdmi")
VIDEO_FN(omt_status_video_retrying, "retrying")
VIDEO_FN(omt_status_unsupported_format, "unsupported-format")
VIDEO_FN(omt_status_video_running, "running")
#undef VIDEO_FN
void omt_status_audio_running(omt_playback_status *s, const char *d, const omt_connector *c) { set_audio(s, "running", d, c); }
void omt_status_audio_failed(omt_playback_status *s, const char *d, const omt_connector *c) { set_audio(s, "failed", d, c); }
void omt_status_audio_stopped(omt_playback_status *s, const omt_connector *c) { set_audio(s, "stopped", "", c); }

void omt_status_heartbeat(omt_playback_status *status, const omt_connector *connector)
{
    (void)pthread_mutex_lock(&status->mutex);
    publish_locked(status, connector, false);
    (void)pthread_mutex_unlock(&status->mutex);
}

void omt_status_stopped(omt_playback_status *status, const char *detail, const omt_connector *connector)
{
    (void)pthread_mutex_lock(&status->mutex);
    (void)omt_copy_string(status->video_state, sizeof(status->video_state), "stopped");
    (void)omt_copy_string(status->audio_state, sizeof(status->audio_state), "stopped");
    (void)omt_sanitize_status_detail(status->video_detail, sizeof(status->video_detail), detail);
    status->audio_detail[0] = '\0';
    publish_locked(status, connector, true);
    (void)pthread_mutex_unlock(&status->mutex);
}
