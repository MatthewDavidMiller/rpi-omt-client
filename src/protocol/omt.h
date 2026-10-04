/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The OMT wire transport and the target grammar shared by the receiver, the
 * Web frontend, and the test sender. Pure parsing and validation, no I/O.
 *
 * The fixed header decides how many bytes the channel reads next, so every
 * length is bounded here before anything is allocated: 10 MiB of video, 1 MiB
 * of audio, 64 KiB of metadata.
 */
#ifndef OMT_PROTOCOL_OMT_H
#define OMT_PROTOCOL_OMT_H

#include "common/base.h"
#include "common/buf.h"

#define OMT_HEADER_SIZE 16u
#define OMT_VIDEO_HEADER_SIZE 32u
#define OMT_AUDIO_HEADER_SIZE 24u
#define OMT_VIDEO_MAX_SIZE (10u * 1024u * 1024u)
#define OMT_AUDIO_MAX_SIZE (1024u * 1024u)
#define OMT_METADATA_MAX_SIZE (64u * 1024u)
#define OMT_SOURCE_NAME_MAX_BYTES 63u
#define OMT_TARGET_MAX_BYTES 512u
#define OMT_HOST_MAX_BYTES 256u

typedef enum {
    OMT_FRAME_METADATA = 1,
    OMT_FRAME_VIDEO = 2,
    OMT_FRAME_AUDIO = 4,
} omt_frame_type;

#define OMT_CODEC_VMX1 0x31584d56
#define OMT_CODEC_FPA1 0x31415046

typedef struct {
    omt_frame_type frame_type;
    int64_t timestamp;
    uint16_t metadata_length;
    uint32_t data_length;
} omt_frame_header;

typedef struct {
    int32_t codec;
    int32_t width;
    int32_t height;
    int32_t frame_rate_n;
    int32_t frame_rate_d;
    float aspect_ratio;
    uint32_t flags;
    int32_t color_space;
} omt_video_header;

typedef struct {
    int32_t codec;
    int32_t sample_rate;
    int32_t samples_per_channel;
    int32_t channels;
    uint32_t active_channels;
} omt_audio_header;

typedef struct {
    char host[OMT_HOST_MAX_BYTES];
    uint16_t port;
} omt_direct_target;

typedef enum {
    OMT_PROTO_OK = 0,
    OMT_PROTO_TRUNCATED,
    OMT_PROTO_UNSUPPORTED,
    OMT_PROTO_OVERSIZED,
    OMT_PROTO_INVALID,
} omt_proto_kind;

/* A protocol error: its kind and the static detail Rust displayed after it,
 * e.g. "unsupported OMT frame version". */
typedef struct {
    omt_proto_kind kind;
    const char *detail;
} omt_proto_error;

void omt_proto_error_format(const omt_proto_error *e, char *out, size_t size);

OMT_NODISCARD bool omt_parse_frame_header(const uint8_t *data, size_t len, omt_frame_header *out,
                                          omt_proto_error *err);
OMT_NODISCARD bool omt_parse_video_header(const omt_frame_header *frame, const uint8_t *data,
                                          size_t len, omt_video_header *out, omt_proto_error *err);
OMT_NODISCARD bool omt_parse_audio_header(const omt_frame_header *frame, const uint8_t *data,
                                          size_t len, omt_audio_header *out, omt_proto_error *err);
/* Builds a metadata frame carrying `xml` into `out`. */
OMT_NODISCARD bool omt_build_metadata(const char *xml, size_t len, int64_t timestamp, omt_buf *out,
                                      omt_proto_error *err);

OMT_NODISCARD bool omt_parse_direct_target(const char *value, size_t len, omt_direct_target *out,
                                           omt_proto_error *err);
bool omt_is_valid_source_name(const char *value, size_t len);
bool omt_is_valid_target(const char *value, size_t len);
/* True for a literal no connect can use: unspecified, multicast, broadcast,
 * and IPv6 link-local (which needs a zone the grammar cannot carry). */
bool omt_is_undiallable_ipv4(const uint8_t a[4]);
bool omt_is_undiallable_ipv6(const uint8_t a[16]);

/* The forbidden source-name code points (Cc, Cf, Cs, Zl, Zp), as sorted,
 * inclusive ranges. Surrogates are listed too even though valid UTF-8 cannot
 * carry them, so this table equals the published one exactly. */
typedef struct {
    uint32_t low;
    uint32_t high;
} omt_codepoint_range;
extern const omt_codepoint_range omt_forbidden_name_ranges[];
extern const size_t omt_forbidden_name_range_count;
bool omt_is_forbidden_name_char(uint32_t cp);

#endif
