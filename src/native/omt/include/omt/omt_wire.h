/*
 * Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The wire definitions are derived from the MIT-licensed libomtnet project.
 */
#ifndef OMT_WIRE_H
#define OMT_WIRE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define OMT_WIRE_HEADER_SIZE 16u
#define OMT_WIRE_VIDEO_HEADER_SIZE 32u
#define OMT_WIRE_AUDIO_HEADER_SIZE 24u
#define OMT_WIRE_VIDEO_MAX_SIZE (10u * 1024u * 1024u)
#define OMT_WIRE_AUDIO_MAX_SIZE (1u * 1024u * 1024u)
#define OMT_WIRE_METADATA_MAX_SIZE (64u * 1024u)
#define OMT_SOURCE_NAME_MAX_BYTES 63u
#define OMT_TARGET_MAX_BYTES 512u

typedef enum omt_frame_type {
    OMT_FRAME_NONE = 0,
    OMT_FRAME_METADATA = 1,
    OMT_FRAME_VIDEO = 2,
    OMT_FRAME_AUDIO = 4
} omt_frame_type;

typedef enum omt_codec {
    OMT_CODEC_VMX1 = 0x31584d56,
    OMT_CODEC_FPA1 = 0x31415046,
    OMT_CODEC_UYVY = 0x59565955,
    OMT_CODEC_YUY2 = 0x32595559,
    OMT_CODEC_BGRA = 0x41524742,
    OMT_CODEC_NV12 = 0x3231564e,
    OMT_CODEC_YV12 = 0x32315659,
    OMT_CODEC_UYVA = 0x41565955,
    OMT_CODEC_P216 = 0x36313250,
    OMT_CODEC_PA16 = 0x36314150
} omt_codec;

typedef struct omt_frame_header {
    uint8_t version;
    omt_frame_type type;
    int64_t timestamp;
    uint16_t metadata_length;
    uint32_t data_length;
} omt_frame_header;

typedef struct omt_video_header {
    omt_codec codec;
    int32_t width;
    int32_t height;
    int32_t frame_rate_n;
    int32_t frame_rate_d;
    float aspect_ratio;
    uint32_t flags;
    int32_t color_space;
} omt_video_header;

typedef struct omt_audio_header {
    omt_codec codec;
    int32_t sample_rate;
    int32_t samples_per_channel;
    int32_t channels;
    uint32_t active_channels;
} omt_audio_header;

typedef struct omt_direct_target {
    char host[256];
    uint16_t port;
    bool ipv6_literal;
} omt_direct_target;

bool omt_wire_parse_header(
    const uint8_t *data,
    size_t length,
    omt_frame_header *header,
    char *error,
    size_t error_length);

bool omt_wire_parse_video_header(
    const omt_frame_header *frame,
    const uint8_t *data,
    size_t length,
    omt_video_header *video,
    char *error,
    size_t error_length);

bool omt_wire_parse_audio_header(
    const omt_frame_header *frame,
    const uint8_t *data,
    size_t length,
    omt_audio_header *audio,
    char *error,
    size_t error_length);

bool omt_wire_build_metadata(
    const char *xml,
    int64_t timestamp,
    uint8_t *output,
    size_t capacity,
    size_t *written);

bool omt_parse_direct_target(const char *value, omt_direct_target *target);
bool omt_is_valid_source_name_utf8(const char *value);

#ifdef __cplusplus
}
#endif

#endif
