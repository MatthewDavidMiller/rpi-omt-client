// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
// Playback code is derived from the MIT-licensed Open Media Transport projects.
#define _GNU_SOURCE
#include "alsa_output.h"
#include "discovery.h"
#include "drm_output.h"
#include "json_text.h"
#include "omt_channel.h"
#include "playback_status.h"

#include <errno.h>
#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifndef OMT_CLIENT_VERSION
#define OMT_CLIENT_VERSION "unknown"
#endif

#define MAX_OPTIONS 8u
#define DISCOVERY_JSON_CAPACITY (128u * 1024u)

static volatile sig_atomic_t running = 1;

static void signal_handler(int signal_number)
{
    (void)signal_number;
    running = 0;
}

typedef struct {
    const char *key;
    const char *value;
    bool flag;
} option;

typedef struct {
    option values[MAX_OPTIONS];
    size_t count;
} options;

static bool parse_options(int argc, char **argv, int begin, options *result,
                          char *error, size_t capacity)
{
    for (int index = begin; index < argc; ++index) {
        const char *key = argv[index];
        if (strncmp(key, "--", 2u) != 0) {
            (void)snprintf(error, capacity, "Unexpected argument: %s", key);
            return false;
        }
        bool flag = strcmp(key, "--json") == 0;
        const char *value = NULL;
        if (!flag && ++index >= argc) {
            (void)snprintf(error, capacity, "Missing value for %s", key);
            return false;
        }
        if (!flag) value = argv[index];
        for (size_t i = 0u; i < result->count; ++i) {
            if (strcmp(result->values[i].key, key) == 0) {
                (void)snprintf(error, capacity, "Duplicate option: %s", key);
                return false;
            }
        }
        if (result->count == MAX_OPTIONS) {
            omt_set_error(error, capacity, "Too many options.");
            return false;
        }
        result->values[result->count++] = (option){key, value, flag};
    }
    return true;
}

static const option *find_option(const options *values, const char *name)
{
    for (size_t i = 0u; i < values->count; ++i)
        if (strcmp(values->values[i].key, name) == 0) return &values->values[i];
    return NULL;
}

static bool allowed(const options *values, const char *const *names, size_t count,
                    char *error, size_t capacity)
{
    for (size_t i = 0u; i < values->count; ++i) {
        bool found = false;
        for (size_t j = 0u; j < count; ++j) found |= strcmp(values->values[i].key, names[j]) == 0;
        if (!found) {
            (void)snprintf(error, capacity, "Option %s is not valid for this command.",
                           values->values[i].key);
            return false;
        }
    }
    return true;
}

static const char *required(const options *values, const char *name, char *error, size_t capacity)
{
    const option *found = find_option(values, name);
    if (found == NULL || found->flag || found->value == NULL || found->value[0] == '\0') {
        (void)snprintf(error, capacity, "%s is required.", name);
        return NULL;
    }
    return found->value;
}

static bool required_flag(const options *values, const char *name, char *error, size_t capacity)
{
    const option *found = find_option(values, name);
    if (found == NULL || !found->flag) {
        (void)snprintf(error, capacity, "%s is required.", name);
        return false;
    }
    return true;
}

static bool integer_option(const options *values, const char *name, int default_value,
                           int minimum, int maximum, int *output, char *error, size_t capacity)
{
    const option *found = find_option(values, name);
    if (found == NULL) {
        *output = default_value;
        return true;
    }
    if (found->flag || found->value == NULL) {
        (void)snprintf(error, capacity, "%s requires a value.", name);
        return false;
    }
    char *end = NULL;
    errno = 0;
    long value = strtol(found->value, &end, 10);
    if (errno != 0 || end == found->value || *end != '\0' || value < minimum || value > maximum) {
        (void)snprintf(error, capacity, "%s must be between %d and %d.", name, minimum, maximum);
        return false;
    }
    *output = (int)value;
    return true;
}

static bool valid_target(const char *target)
{
    omt_direct_target direct;
    return strncmp(target, "omt://", 6u) == 0
        ? omt_parse_direct_target(target, &direct) : omt_is_valid_source_name_utf8(target);
}

static bool direct_target(const char *target)
{
    omt_direct_target direct;
    return omt_parse_direct_target(target, &direct);
}

static bool emit_json(const char *document)
{
    if (fputs(document, stdout) >= 0 && fflush(stdout) == 0) return true;
    (void)fputs("Unable to write the JSON result to standard output.\n", stderr);
    return false;
}

static void interruptible_wait(int milliseconds, omt_playback_status *status,
                               const omt_connector *connector)
{
    int remaining = milliseconds;
    while (running != 0 && remaining > 0) {
        int slice = remaining < 100 ? remaining : 100;
        omt_sleep_milliseconds(slice);
        remaining -= slice;
        if (status != NULL) omt_status_heartbeat(status, connector);
    }
}

static int discover_command(const options *values, char *error, size_t capacity)
{
    static const char *const names[] = {"--wait-ms", "--json"};
    int wait_ms = 0;
    if (!allowed(values, names, 2u, error, capacity) ||
        !required_flag(values, "--json", error, capacity) ||
        !integer_option(values, "--wait-ms", 1500, 0, 60000, &wait_ms, error, capacity)) return 2;
    omt_source sources[OMT_MAX_SOURCES];
    size_t count = omt_discover_sources(sources, OMT_MAX_SOURCES, wait_ms);
    char *document = malloc(DISCOVERY_JSON_CAPACITY);
    if (document == NULL) {
        omt_set_error(error, capacity, "Unable to allocate discovery result.");
        return 1;
    }
    omt_text_buffer text;
    omt_text_init(&text, document, DISCOVERY_JSON_CAPACITY);
    (void)omt_text_append_char(&text, '[');
    for (size_t i = 0u; i < count; ++i) {
        if (i != 0u) (void)omt_text_append_char(&text, ',');
        (void)omt_text_append(&text, "{\"name\":");
        (void)omt_json_append_string(&text, sources[i].name);
        (void)omt_text_append(&text, ",\"target\":");
        (void)omt_json_append_string(&text, sources[i].name);
        (void)omt_text_append(&text, ",\"kind\":\"discovered\"}");
    }
    (void)omt_text_append(&text, "]\n");
    bool emitted = !text.failed && emit_json(document);
    free(document);
    return emitted ? 0 : 1;
}

static bool next_media_frame(omt_channel *channel, omt_frame_type wanted, omt_frame *frame,
                             omt_deadline deadline, char *error, size_t capacity)
{
    while (omt_remaining_milliseconds(deadline) > 0) {
        if (!omt_channel_receive(channel, frame, deadline, error, capacity)) return false;
        if (frame->header.type == wanted) return true;
    }
    omt_set_error(error, capacity, "OMT media deadline expired");
    return false;
}

static omt_deadline earlier_deadline(omt_deadline left, omt_deadline right)
{
    if (left.value.tv_sec < right.value.tv_sec ||
        (left.value.tv_sec == right.value.tv_sec && left.value.tv_nsec < right.value.tv_nsec)) return left;
    return right;
}

static int probe_command(const options *values, char *error, size_t capacity)
{
    static const char *const names[] = {"--target", "--timeout-ms", "--json"};
    int timeout_ms = 0;
    if (!allowed(values, names, 3u, error, capacity) ||
        !required_flag(values, "--json", error, capacity)) return 2;
    const char *target = required(values, "--target", error, capacity);
    if (!integer_option(values, "--timeout-ms", 3000, 1, 60000, &timeout_ms, error, capacity) ||
        target == NULL || !valid_target(target)) {
        if (error[0] == '\0') omt_set_error(error, capacity, "Invalid OMT direct target.");
        return 2;
    }
    omt_endpoint endpoint;
    bool resolved = omt_resolve_target(target, timeout_ms, &endpoint);
    omt_deadline deadline = omt_deadline_after_ms(timeout_ms);
    bool video = false, audio = false;
    int width = 0, height = 0, channels = 0, sample_rate = 0;
    double frame_rate = 0.0;
    char probe_error[OMT_ERROR_CAPACITY] = {0};
    omt_channel *video_channel = omt_channel_create();
    omt_channel *audio_channel = omt_channel_create();
    omt_frame frame;
    omt_frame_init(&frame);
    if (!resolved) {
        omt_set_error(probe_error, sizeof(probe_error), "OMT target was not discovered.");
    } else if (video_channel == NULL || audio_channel == NULL) {
        omt_set_error(probe_error, sizeof(probe_error), "Unable to allocate OMT channels.");
    } else {
        /* Scratch for the connect and per-slice reads. A slice that expires
           before the first frame leaves a message here that must not be
           reported once any media does arrive. */
        char channel_error[OMT_ERROR_CAPACITY] = {0};
        bool video_connected = omt_channel_connect(video_channel, &endpoint, OMT_FRAME_VIDEO,
                                                    deadline, channel_error, sizeof(channel_error));
        bool audio_connected = omt_channel_connect(audio_channel, &endpoint, OMT_FRAME_AUDIO,
                                                    deadline, channel_error, sizeof(channel_error));
        while (omt_remaining_milliseconds(deadline) > 0 && !(video && audio)) {
            omt_deadline slice = earlier_deadline(deadline, omt_deadline_after_ms(100));
            if (!video && video_connected && next_media_frame(video_channel, OMT_FRAME_VIDEO,
                    &frame, slice, channel_error, sizeof(channel_error))) {
                video = true; width = frame.video.width; height = frame.video.height;
                frame_rate = (double)frame.video.frame_rate_n / (double)frame.video.frame_rate_d;
            }
            slice = earlier_deadline(deadline, omt_deadline_after_ms(video ? 100 : 1));
            if (!audio && audio_connected && next_media_frame(audio_channel, OMT_FRAME_AUDIO,
                    &frame, slice, channel_error, sizeof(channel_error))) {
                audio = true; channels = frame.audio.channels; sample_rate = frame.audio.sample_rate;
            }
        }
        if (!(video || audio))
            omt_set_error(probe_error, sizeof(probe_error),
                          channel_error[0] == '\0' ? "No OMT media was received." : channel_error);
    }
    char clean_error[OMT_ERROR_CAPACITY];
    (void)omt_sanitize_status_detail(clean_error, sizeof(clean_error), probe_error);
    char document[4096];
    omt_text_buffer text;
    omt_text_init(&text, document, sizeof(document));
    (void)omt_text_append(&text, video || audio ? "{\"ok\":true,\"target\":" : "{\"ok\":false,\"target\":");
    (void)omt_json_append_string(&text, target);
    char measurements[256];
    (void)snprintf(measurements, sizeof(measurements),
        ",\"video\":%s,\"audio\":%s,\"width\":%d,\"height\":%d,"
        "\"frame_rate\":%.8g,\"channels\":%d,\"sample_rate\":%d,\"error\":",
        video ? "true" : "false", audio ? "true" : "false", width, height,
        frame_rate, channels, sample_rate);
    (void)omt_text_append(&text, measurements);
    (void)omt_json_append_string(&text, clean_error);
    (void)omt_text_append(&text, "}\n");
    omt_frame_destroy(&frame);
    omt_channel_destroy(video_channel);
    omt_channel_destroy(audio_channel);
    if (text.failed || !emit_json(document)) return 1;
    return video || audio ? 0 : 3;
}

typedef struct {
    omt_endpoint endpoint;
    omt_connector connector;
    omt_playback_status *status;
    atomic_bool active;
    pthread_t thread;
    bool started;
} audio_worker;

static void *audio_entry(void *argument)
{
    audio_worker *worker = argument;
    while (atomic_load_explicit(&worker->active, memory_order_relaxed) && running != 0) {
        omt_channel *channel = omt_channel_create();
        omt_alsa_output *output = omt_alsa_output_create();
        omt_frame frame;
        omt_frame_init(&frame);
        char error[OMT_ERROR_CAPACITY] = {0};
        if (channel != NULL && output != NULL && omt_channel_connect(
                channel, &worker->endpoint, OMT_FRAME_AUDIO, omt_deadline_after_ms(3000),
                error, sizeof(error))) {
            while (atomic_load_explicit(&worker->active, memory_order_relaxed) && running != 0) {
                if (!next_media_frame(channel, OMT_FRAME_AUDIO, &frame, omt_deadline_after_ms(100),
                                      error, sizeof(error))) {
                    if (omt_channel_connected(channel)) continue;
                    break;
                }
                if (!omt_alsa_output_write(output, &frame, worker->connector.alsa_device,
                                           error, sizeof(error))) break;
                omt_status_audio_running(worker->status, "Playing OMT video and audio.", &worker->connector);
            }
        }
        omt_frame_destroy(&frame);
        omt_alsa_output_destroy(output);
        omt_channel_destroy(channel);
        if (atomic_load_explicit(&worker->active, memory_order_relaxed) && running != 0) {
            char detail[OMT_ERROR_CAPACITY];
            char clean[OMT_ERROR_CAPACITY];
            (void)omt_sanitize_status_detail(clean, sizeof(clean), error);
            (void)snprintf(detail, sizeof(detail), "Audio unavailable: %.2028s", clean);
            omt_status_audio_failed(worker->status, detail, &worker->connector);
            for (int i = 0; i < 10 && atomic_load_explicit(&worker->active, memory_order_relaxed) &&
                 running != 0; ++i) omt_sleep_milliseconds(100);
        }
    }
    omt_status_audio_stopped(worker->status, &worker->connector);
    return NULL;
}

static bool audio_start(audio_worker *worker)
{
    atomic_store_explicit(&worker->active, true, memory_order_relaxed);
    pthread_attr_t attributes;
    if (pthread_attr_init(&attributes) != 0) return false;
    size_t stack_size = 512u * 1024u;
#if defined(PTHREAD_STACK_MIN)
    if ((size_t)PTHREAD_STACK_MIN > stack_size) stack_size = (size_t)PTHREAD_STACK_MIN;
#endif
    int result = pthread_attr_setstacksize(&attributes, stack_size);
    if (result == 0) result = pthread_create(&worker->thread, &attributes, audio_entry, worker);
    (void)pthread_attr_destroy(&attributes);
    if (result != 0) {
        atomic_store_explicit(&worker->active, false, memory_order_relaxed);
        omt_status_audio_failed(worker->status,
            "Audio unavailable: unable to create bounded-stack worker.", &worker->connector);
        return false;
    }
    worker->started = true;
    return true;
}

static void audio_stop(audio_worker *worker)
{
    atomic_store_explicit(&worker->active, false, memory_order_relaxed);
    if (worker->started) {
        (void)pthread_join(worker->thread, NULL);
        worker->started = false;
    }
}

static bool run_session(const char *target, const omt_connector *connector,
                        omt_playback_status *status, char *error, size_t capacity)
{
    omt_endpoint endpoint;
    if (!omt_resolve_target(target, 1500, &endpoint)) {
        omt_set_error(error, capacity, "OMT target was not discovered.");
        return false;
    }
    omt_drm_output *output = omt_drm_output_create(connector);
    omt_channel *video = omt_channel_create();
    if (!omt_drm_output_ready(output)) {
        omt_set_error(error, capacity, omt_drm_output_error(output));
        omt_drm_output_destroy(output); omt_channel_destroy(video);
        return false;
    }
    if (video == NULL || !omt_channel_connect(video, &endpoint, OMT_FRAME_VIDEO,
                                               omt_deadline_after_ms(3000), error, capacity)) {
        omt_drm_output_destroy(output); omt_channel_destroy(video);
        return false;
    }
    audio_worker audio;
    memset(&audio, 0, sizeof(audio));
    audio.endpoint = endpoint; audio.connector = *connector; audio.status = status;
    (void)audio_start(&audio);
    omt_status_video_starting(status, "Waiting for OMT media.", connector);
    omt_frame frame;
    omt_frame_init(&frame);
    /* A fresh connection gets the same five-second grace as one that has been
       delivering frames, so "starting" stays visible while the sender ramps up. */
    omt_deadline last_frame = omt_deadline_after_ms(5000);
    omt_deadline connector_check = omt_deadline_after_ms(0);
    while (running != 0) {
        if (omt_remaining_milliseconds(connector_check) == 0) {
            connector_check = omt_deadline_after_ms(500);
            if (!omt_connector_is_connected(connector)) {
                omt_set_error(error, capacity, "HDMI display disconnected.");
                break;
            }
        }
        char receive_error[OMT_ERROR_CAPACITY] = {0};
        if (!next_media_frame(video, OMT_FRAME_VIDEO, &frame, omt_deadline_after_ms(500),
                              receive_error, sizeof(receive_error))) {
            omt_status_heartbeat(status, connector);
            if (!omt_channel_connected(video)) {
                omt_set_error(error, capacity, receive_error);
                break;
            }
            if (omt_remaining_milliseconds(last_frame) == 0)
                omt_status_video_retrying(status, "Waiting for video frames.", connector);
            continue;
        }
        last_frame = omt_deadline_after_ms(5000);
        omt_present_outcome outcome = omt_drm_output_present(output, &frame);
        if (outcome == OMT_PRESENT_UNSUPPORTED_FORMAT) {
            omt_status_unsupported_format(status, omt_drm_output_error(output), connector);
            continue;
        }
        if (outcome == OMT_PRESENT_FAILED) {
            omt_set_error(error, capacity, omt_drm_output_error(output));
            break;
        }
        omt_status_video_running(status, (frame.video.flags & 1u) != 0u
            ? "Playing interlaced input progressively without deinterlacing."
            : "Playing OMT video.", connector);
    }
    audio_stop(&audio);
    omt_status_audio_stopped(status, connector);
    omt_frame_destroy(&frame);
    omt_channel_destroy(video);
    omt_drm_output_destroy(output);
    return error[0] == '\0';
}

static int play_command(const options *values, char *error, size_t capacity)
{
    static const char *const names[] = {"--target", "--connector", "--status-file", "--retry-seconds"};
    if (!allowed(values, names, 4u, error, capacity)) return 2;
    const char *target = required(values, "--target", error, capacity);
    const char *status_file = required(values, "--status-file", error, capacity);
    int retry = 0;
    if (!integer_option(values, "--retry-seconds", 2, 1, 30, &retry, error, capacity)) return 2;
    const option *connector_option = find_option(values, "--connector");
    const char *preference = connector_option == NULL ? "auto" : connector_option->value;
    if (target == NULL || status_file == NULL || !valid_target(target) || preference == NULL ||
        (strcmp(preference, "auto") != 0 && strcmp(preference, "HDMI-A-1") != 0 &&
         strcmp(preference, "HDMI-A-2") != 0)) {
        if (error[0] == '\0') omt_set_error(error, capacity, "Invalid play options.");
        return 2;
    }
    omt_playback_status *status = omt_playback_status_create(status_file, target);
    if (status == NULL) {
        omt_set_error(error, capacity, "Unable to initialize playback status.");
        return 1;
    }
    while (running != 0) {
        if (!direct_target(target) && !omt_discovery_transport_available()) {
            omt_status_waiting_for_discovery(status,
                "No configured OMT discovery transport is available.", NULL);
            interruptible_wait(1000, status, NULL);
            continue;
        }
        omt_connector connector;
        if (!omt_find_connector(preference, &connector)) {
            omt_status_waiting_for_hdmi(status, "No supported HDMI display is connected.", NULL);
            interruptible_wait(1000, status, NULL);
            continue;
        }
        char session_error[OMT_ERROR_CAPACITY] = {0};
        if (!run_session(target, &connector, status, session_error, sizeof(session_error)) && running != 0) {
            char clean[OMT_ERROR_CAPACITY];
            (void)omt_sanitize_status_detail(clean, sizeof(clean), session_error);
            omt_status_video_retrying(status, clean, &connector);
            interruptible_wait(retry * 1000, status, &connector);
        }
    }
    omt_status_stopped(status, "Playback stopped.", NULL);
    omt_playback_status_destroy(status);
    return 0;
}

static int usage(void)
{
    (void)fputs("Usage: omt-receiver --version | discover --wait-ms N --json | "
        "probe --target TARGET --timeout-ms N --json | "
        "play --target TARGET --connector auto|HDMI-A-1|HDMI-A-2 --status-file PATH\n", stderr);
    return 2;
}

int main(int argc, char **argv)
{
    (void)signal(SIGINT, signal_handler);
    (void)signal(SIGTERM, signal_handler);
    if (argc == 2 && strcmp(argv[1], "--version") == 0) {
        (void)puts(OMT_CLIENT_VERSION);
        return 0;
    }
    if (argc < 2) return usage();
    options values;
    memset(&values, 0, sizeof(values));
    char error[OMT_ERROR_CAPACITY] = {0};
    if (!parse_options(argc, argv, 2, &values, error, sizeof(error))) {
        (void)fprintf(stderr, "%s\n", error);
        return 2;
    }
    int result = 2;
    if (strcmp(argv[1], "discover") == 0) result = discover_command(&values, error, sizeof(error));
    else if (strcmp(argv[1], "probe") == 0) result = probe_command(&values, error, sizeof(error));
    else if (strcmp(argv[1], "play") == 0) result = play_command(&values, error, sizeof(error));
    else return usage();
    if (result == 2 && error[0] != '\0') (void)fprintf(stderr, "%s\n", error);
    return result;
}
