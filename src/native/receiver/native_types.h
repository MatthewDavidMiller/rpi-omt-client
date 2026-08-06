// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#ifndef OMT_RECEIVER_NATIVE_TYPES_H
#define OMT_RECEIVER_NATIVE_TYPES_H

#include "omt/omt_wire.h"

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <time.h>

#define OMT_ENDPOINT_HOST_CAPACITY 256u
#define OMT_SOURCE_NAME_CAPACITY 64u
#define OMT_CONNECTOR_NAME_CAPACITY 16u
#define OMT_PATH_CAPACITY 4096u
#define OMT_ALSA_DEVICE_CAPACITY 128u
#define OMT_ERROR_CAPACITY 2049u
#define OMT_MAX_SOURCES 256u

typedef struct {
    char host[OMT_ENDPOINT_HOST_CAPACITY];
    uint16_t port;
} omt_endpoint;

typedef struct {
    char name[OMT_SOURCE_NAME_CAPACITY];
    omt_endpoint endpoint;
} omt_source;

typedef struct {
    omt_frame_header header;
    omt_video_header video;
    omt_audio_header audio;
    uint8_t *payload;
    size_t payload_capacity;
} omt_frame;

typedef struct {
    char name[OMT_CONNECTOR_NAME_CAPACITY];
    char device_path[OMT_PATH_CAPACITY];
    char sysfs_path[OMT_PATH_CAPACITY];
    uint32_t connector_id;
    char alsa_device[OMT_ALSA_DEVICE_CAPACITY];
} omt_connector;

typedef struct {
    struct timespec value;
} omt_deadline;

omt_deadline omt_deadline_after_ms(int milliseconds);
int omt_remaining_milliseconds(omt_deadline deadline);
void omt_sleep_milliseconds(int milliseconds);
bool omt_copy_string(char *destination, size_t capacity, const char *source);
void omt_set_error(char *error, size_t capacity, const char *message);
void omt_frame_init(omt_frame *frame);
void omt_frame_destroy(omt_frame *frame);

#endif
