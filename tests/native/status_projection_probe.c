// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#define _POSIX_C_SOURCE 200809L
#include "playback_status.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

int main(int argc, char **argv)
{
    char directory[] = "/tmp/omt-status-vector-XXXXXX";
    if (mkdtemp(directory) == NULL) return 1;
    char path[4096];
    (void)snprintf(path, sizeof(path), "%s/status.json", directory);
    omt_playback_status *status = omt_playback_status_create(path, "Camera");
    if (status == NULL) return 1;
    omt_status_heartbeat(status, NULL);
    for (int index = 1; index < argc; ++index) {
        const char *event = argv[index];
        if (strcmp(event, "AudioRunning") == 0) omt_status_audio_running(status, "audio", NULL);
        else if (strcmp(event, "VideoStarting") == 0) omt_status_video_starting(status, "starting", NULL);
        else if (strcmp(event, "WaitingForDiscovery") == 0) omt_status_waiting_for_discovery(status, "discovery", NULL);
        else if (strcmp(event, "WaitingForHdmi") == 0) omt_status_waiting_for_hdmi(status, "hdmi", NULL);
        else if (strcmp(event, "VideoRetrying") == 0) omt_status_video_retrying(status, "retrying", NULL);
        else if (strcmp(event, "UnsupportedFormat") == 0) omt_status_unsupported_format(status, "unsupported", NULL);
        else if (strcmp(event, "VideoRunning") == 0) omt_status_video_running(status, "video", NULL);
        else if (strcmp(event, "AudioFailed") == 0) omt_status_audio_failed(status, "audio failed", NULL);
        else if (strcmp(event, "AudioStopped") == 0) omt_status_audio_stopped(status, NULL);
        else if (strcmp(event, "Stopped") == 0) omt_status_stopped(status, "stopped", NULL);
        else return 2;
    }
    FILE *input = fopen(path, "rb");
    if (input == NULL) return 1;
    char buffer[4096];
    size_t count;
    while ((count = fread(buffer, 1u, sizeof(buffer), input)) != 0u)
        if (fwrite(buffer, 1u, count, stdout) != count) return 1;
    (void)fclose(input);
    omt_playback_status_destroy(status);
    (void)unlink(path);
    (void)rmdir(directory);
    return ferror(stdout) ? 1 : 0;
}
