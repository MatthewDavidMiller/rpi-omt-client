// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#define _POSIX_C_SOURCE 200809L
#include "json_text.h"
#include "playback_status.h"

#include <dirent.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static void require(bool value, const char *message)
{
    if (!value) {
        (void)fprintf(stderr, "FAIL: %s\n", message);
        exit(1);
    }
}

static size_t read_file(const char *path, char *output, size_t capacity)
{
    FILE *input = fopen(path, "rb");
    if (input == NULL) return 0u;
    size_t count = fread(output, 1u, capacity - 1u, input);
    output[count] = '\0';
    (void)fclose(input);
    return count;
}

static size_t count_entries(const char *directory)
{
    DIR *entries = opendir(directory);
    require(entries != NULL, "test directory opens");
    size_t count = 0u;
    for (struct dirent *entry = readdir(entries); entry != NULL; entry = readdir(entries))
        if (strcmp(entry->d_name, ".") != 0 && strcmp(entry->d_name, "..") != 0) count++;
    (void)closedir(entries);
    return count;
}

static void sanitized_equals(const char *input, const char *expected, const char *message)
{
    char output[4097];
    (void)omt_sanitize_status_detail(output, sizeof(output), input);
    require(strcmp(output, expected) == 0, message);
}

static void detail_sanitization_contract(void)
{
    sanitized_equals("Playing OMT video.", "Playing OMT video.", "clean ASCII detail is preserved");
    sanitized_equals("  padded\t\r\n", "padded", "surrounding whitespace is trimmed");
    sanitized_equals(" \t\r\n", "", "an all-whitespace detail becomes empty");
    sanitized_equals("", "", "an empty detail stays empty");
    sanitized_equals("a\nb", "ab", "interior controls are dropped");
    sanitized_equals("a\x7f" "b", "ab", "DEL is dropped");
    sanitized_equals("Caf\xc3\xa9", "Caf\xc3\xa9", "valid UTF-8 is preserved");
    sanitized_equals("a\xc3\x28" "b", "a(b", "parsing resynchronizes after a bad lead");
    sanitized_equals("a\xc3\xc3" "b", "ab", "a bad continuation byte is dropped");
    sanitized_equals("a\xc0\xaf" "b", "ab", "an overlong encoding is dropped");
    sanitized_equals("a\xed\xa0\x80" "b", "ab", "a surrogate is dropped");
    sanitized_equals("a\xf5\x80\x80\x80" "b", "ab", "an above-range scalar is dropped");
    sanitized_equals("a\xff" "b", "ab", "an invalid lead is dropped");
    sanitized_equals("a\xe2\x82", "a", "a truncated tail sequence is dropped");
    char oversized[4097];
    memset(oversized, 'x', sizeof(oversized) - 1u);
    oversized[sizeof(oversized) - 1u] = '\0';
    char result[4097];
    require(omt_sanitize_status_detail(result, sizeof(result), oversized) == 2048u,
            "an oversized detail is truncated");
    char wide[2051];
    memset(wide, 'x', 2047u);
    memcpy(wide + 2047u, "\xe2\x82\xac", 4u);
    require(omt_sanitize_status_detail(result, sizeof(result), wide) == 2047u,
            "a scalar crossing the cap is dropped whole");
}

static void json_escaping_contract(void)
{
    char output[128];
    require(omt_json_string(output, sizeof(output), "plain") && strcmp(output, "\"plain\"") == 0,
            "plain text is quoted");
    require(omt_json_string(output, sizeof(output), "a\"b") && strcmp(output, "\"a\\\"b\"") == 0,
            "a quote is escaped");
    require(omt_json_string(output, sizeof(output), "a\\b") && strcmp(output, "\"a\\\\b\"") == 0,
            "a backslash is escaped");
    require(omt_json_string(output, sizeof(output), "\b\f\n\r\t") &&
            strcmp(output, "\"\\b\\f\\n\\r\\t\"") == 0, "short escapes are used");
    require(omt_json_string(output, sizeof(output), "\x01\x1f") &&
            strcmp(output, "\"\\u0001\\u001f\"") == 0, "C0 controls use unicode escapes");
}

static void publication_contract(const char *directory)
{
    char path[4096];
    (void)snprintf(path, sizeof(path), "%s/playback-status.json", directory);
    omt_connector connector = {
        "HDMI-A-1", "/dev/dri/card1", "/sys/class/drm/card1-HDMI-A-1", 32u,
        "plughw:CARD=vc4hdmi0,DEV=0"
    };
    omt_playback_status *status = omt_playback_status_create(path, "Camera");
    require(status != NULL, "status initializes");
    omt_status_video_running(status, "Playing OMT video.\n", &connector);
    omt_status_audio_failed(status, "Audio\nunavailable", &connector);
    char document[16384];
    (void)read_file(path, document, sizeof(document));
    require(strstr(document, "\"state\":\"degraded\"") != NULL, "degraded state is projected");
    require(strstr(document, "\"video_state\":\"running\"") != NULL, "video remains running");
    require(strstr(document, "\"audio_state\":\"failed\"") != NULL, "audio failure is published");
    require(strstr(document, "Audiounavailable") != NULL, "detail controls are removed");
    require(strstr(document, "\"connector\":\"HDMI-A-1\"") != NULL, "connector is published");
    char before[16384];
    (void)omt_copy_string(before, sizeof(before), document);
    for (int repeat = 0; repeat < 32; ++repeat)
        omt_status_video_running(status, "Playing OMT video.\n", &connector);
    (void)read_file(path, document, sizeof(document));
    require(strcmp(document, before) == 0, "an unchanged event does not republish");
    omt_status_video_retrying(status, "Waiting for video frames.", &connector);
    (void)read_file(path, document, sizeof(document));
    require(strstr(document, "\"video_state\":\"retrying\"") != NULL, "a change publishes immediately");
    (void)omt_copy_string(before, sizeof(before), document);
    omt_sleep_milliseconds(600);
    omt_status_heartbeat(status, &connector);
    (void)read_file(path, document, sizeof(document));
    require(strcmp(document, before) != 0, "heartbeat republishes");
    omt_status_stopped(status, "Playback stopped.", NULL);
    (void)read_file(path, document, sizeof(document));
    require(strstr(document, "\"state\":\"stopped\"") != NULL, "terminal state is forced");
    require(count_entries(directory) == 1u, "no stage remains");
    omt_playback_status_destroy(status);
}

int main(void)
{
    char directory[] = "/tmp/omt-native-status-XXXXXX";
    require(mkdtemp(directory) != NULL, "temporary directory is created");
    detail_sanitization_contract();
    json_escaping_contract();
    publication_contract(directory);
    char path[4096];
    (void)snprintf(path, sizeof(path), "%s/playback-status.json", directory);
    (void)unlink(path);
    (void)rmdir(directory);
    (void)puts("native playback status contracts passed");
    return 0;
}
