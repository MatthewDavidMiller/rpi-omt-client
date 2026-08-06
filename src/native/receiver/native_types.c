// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#define _POSIX_C_SOURCE 200809L
#include "native_types.h"

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

omt_deadline omt_deadline_after_ms(int milliseconds)
{
    omt_deadline deadline = {{0, 0}};
    (void)clock_gettime(CLOCK_MONOTONIC, &deadline.value);
    deadline.value.tv_sec += milliseconds / 1000;
    deadline.value.tv_nsec += (long)(milliseconds % 1000) * 1000000L;
    if (deadline.value.tv_nsec >= 1000000000L) {
        deadline.value.tv_sec++;
        deadline.value.tv_nsec -= 1000000000L;
    }
    return deadline;
}

int omt_remaining_milliseconds(omt_deadline deadline)
{
    struct timespec now = {0, 0};
    (void)clock_gettime(CLOCK_MONOTONIC, &now);
    int64_t seconds = (int64_t)deadline.value.tv_sec - (int64_t)now.tv_sec;
    int64_t nanoseconds = (int64_t)deadline.value.tv_nsec - (int64_t)now.tv_nsec;
    int64_t result = seconds * 1000 + nanoseconds / 1000000;
    if (result <= 0) {
        return 0;
    }
    return result > 60000 ? 60000 : (int)result;
}

void omt_sleep_milliseconds(int milliseconds)
{
    struct timespec delay = {milliseconds / 1000, (long)(milliseconds % 1000) * 1000000L};
    while (nanosleep(&delay, &delay) != 0 && errno == EINTR) {
    }
}

bool omt_copy_string(char *destination, size_t capacity, const char *source)
{
    size_t length = source == NULL ? 0u : strlen(source);
    if (destination == NULL || capacity == 0u || source == NULL || length >= capacity) {
        return false;
    }
    memcpy(destination, source, length + 1u);
    return true;
}

void omt_set_error(char *error, size_t capacity, const char *message)
{
    if (error != NULL && capacity != 0u) {
        (void)snprintf(error, capacity, "%s", message == NULL ? "" : message);
    }
}

void omt_frame_init(omt_frame *frame)
{
    if (frame != NULL) {
        memset(frame, 0, sizeof(*frame));
    }
}

void omt_frame_destroy(omt_frame *frame)
{
    if (frame != NULL) {
        free(frame->payload);
        memset(frame, 0, sizeof(*frame));
    }
}
