/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "protocol/omt.h"

#include <math.h>
#include <stdio.h>
#include <string.h>

#include "common/ipaddr.h"
#include "common/json.h"
#include "common/nfc.h"

static bool fail(omt_proto_error *err, omt_proto_kind kind, const char *detail) {
    if (err) {
        err->kind = kind;
        err->detail = detail;
    }
    return false;
}

void omt_proto_error_format(const omt_proto_error *e, char *out, size_t size) {
    static const char *const prefixes[] = {"ok", "truncated", "unsupported", "oversized",
                                           "invalid"};
    const char *prefix = (size_t)e->kind < OMT_ARRAY_LEN(prefixes) ? prefixes[e->kind] : "error";
    snprintf(out, size, "%s %s", prefix, e->detail ? e->detail : "");
}

bool omt_parse_frame_header(const uint8_t *data, size_t len, omt_frame_header *out,
                            omt_proto_error *err) {
    if (len < OMT_HEADER_SIZE) return fail(err, OMT_PROTO_TRUNCATED, "OMT frame header");
    if (data[0] != 1) return fail(err, OMT_PROTO_UNSUPPORTED, "OMT frame version");
    omt_frame_type type;
    uint32_t extension, maximum;
    switch (data[1]) {
    case 1:
        type = OMT_FRAME_METADATA;
        extension = 0;
        maximum = OMT_METADATA_MAX_SIZE;
        break;
    case 2:
        type = OMT_FRAME_VIDEO;
        extension = OMT_VIDEO_HEADER_SIZE;
        maximum = OMT_VIDEO_MAX_SIZE;
        break;
    case 4:
        type = OMT_FRAME_AUDIO;
        extension = OMT_AUDIO_HEADER_SIZE;
        maximum = OMT_AUDIO_MAX_SIZE;
        break;
    default: return fail(err, OMT_PROTO_UNSUPPORTED, "OMT frame type");
    }
    int64_t timestamp = (int64_t)omt_le64(data + 2);
    uint16_t metadata_length = omt_le16(data + 10);
    uint32_t data_length = omt_le32(data + 12);
    if (data_length < extension)
        return fail(err, OMT_PROTO_INVALID, "OMT frame is shorter than its extended header");
    uint32_t payload = data_length - extension;
    if (payload > maximum) return fail(err, OMT_PROTO_OVERSIZED, "OMT frame payload");
    if (metadata_length > payload)
        return fail(err, OMT_PROTO_INVALID, "OMT metadata length exceeds the payload");
    out->frame_type = type;
    out->timestamp = timestamp;
    out->metadata_length = metadata_length;
    out->data_length = data_length;
    return true;
}

static int32_t le_i32(const uint8_t *p) { return (int32_t)omt_le32(p); }

static float le_f32(const uint8_t *p) {
    uint32_t bits = omt_le32(p);
    float f;
    memcpy(&f, &bits, sizeof(f));
    return f;
}

bool omt_parse_video_header(const omt_frame_header *frame, const uint8_t *data, size_t len,
                            omt_video_header *out, omt_proto_error *err) {
    if (frame->frame_type != OMT_FRAME_VIDEO || len < OMT_VIDEO_HEADER_SIZE)
        return fail(err, OMT_PROTO_TRUNCATED, "OMT video header");
    omt_video_header v = {
        .codec = le_i32(data),
        .width = le_i32(data + 4),
        .height = le_i32(data + 8),
        .frame_rate_n = le_i32(data + 12),
        .frame_rate_d = le_i32(data + 16),
        .aspect_ratio = le_f32(data + 20),
        .flags = omt_le32(data + 24),
        .color_space = le_i32(data + 28),
    };
    if (v.width < 16 || v.width > 1920 || v.height < 16 || v.height > 1080 || v.frame_rate_n <= 0 ||
        v.frame_rate_d <= 0)
        return fail(err, OMT_PROTO_UNSUPPORTED, "OMT video dimensions or frame rate");
    double rate = (double)v.frame_rate_n / (double)v.frame_rate_d;
    if (!(rate >= 0.0 && rate <= 60.0) || v.codec != OMT_CODEC_VMX1)
        return fail(err, OMT_PROTO_UNSUPPORTED, "OMT video format");
    if (!isfinite(v.aspect_ratio) || !(v.aspect_ratio >= 0.0f && v.aspect_ratio <= 10.0f) ||
        !(v.color_space == 0 || v.color_space == 601 || v.color_space == 709) ||
        (v.flags & ~31u) != 0)
        return fail(err, OMT_PROTO_UNSUPPORTED, "OMT video properties");
    *out = v;
    return true;
}

bool omt_parse_audio_header(const omt_frame_header *frame, const uint8_t *data, size_t len,
                            omt_audio_header *out, omt_proto_error *err) {
    if (frame->frame_type != OMT_FRAME_AUDIO || len < OMT_AUDIO_HEADER_SIZE)
        return fail(err, OMT_PROTO_TRUNCATED, "OMT audio header");
    omt_audio_header a = {
        .codec = le_i32(data),
        .sample_rate = le_i32(data + 4),
        .samples_per_channel = le_i32(data + 8),
        .channels = le_i32(data + 12),
        .active_channels = omt_le32(data + 16),
    };
    if (a.codec != OMT_CODEC_FPA1 || a.sample_rate < 8000 || a.sample_rate > 192000 ||
        a.channels < 1 || a.channels > 32 || a.samples_per_channel < 1)
        return fail(err, OMT_PROTO_UNSUPPORTED, "OMT audio format");
    uint64_t channels = (uint64_t)a.channels;
    uint64_t samples = (uint64_t)a.samples_per_channel;
    /* Both factors are bounded (32 and 2^31), so the products cannot wrap. */
    uint64_t decoded = samples * channels * 4u;
    if (decoded > OMT_AUDIO_MAX_SIZE) return fail(err, OMT_PROTO_OVERSIZED, "decoded audio");
    uint32_t allowed = channels == 32 ? UINT32_MAX : (uint32_t)((1u << channels) - 1u);
    if (a.active_channels & ~allowed) return fail(err, OMT_PROTO_INVALID, "active audio channel");
    uint64_t compressed = (uint64_t)__builtin_popcount(a.active_channels) * samples * 4u;
    if (frame->data_length < OMT_AUDIO_HEADER_SIZE)
        return fail(err, OMT_PROTO_INVALID, "audio payload");
    uint64_t payload = frame->data_length - OMT_AUDIO_HEADER_SIZE;
    if (compressed + frame->metadata_length != payload)
        return fail(err, OMT_PROTO_INVALID, "OMT planar audio payload length");
    *out = a;
    return true;
}

bool omt_build_metadata(const char *xml, size_t len, int64_t timestamp, omt_buf *out,
                        omt_proto_error *err) {
    if (len == 0 || len >= OMT_METADATA_MAX_SIZE)
        return fail(err, OMT_PROTO_INVALID, "metadata size");
    uint8_t header[OMT_HEADER_SIZE];
    header[0] = 1;
    header[1] = OMT_FRAME_METADATA;
    omt_put_le64(header + 2, (uint64_t)timestamp);
    omt_put_le16(header + 10, 0);
    omt_put_le32(header + 12, (uint32_t)len);
    omt_buf_append(out, header, sizeof(header));
    omt_buf_append(out, xml, len);
    if (out->failed) return fail(err, OMT_PROTO_OVERSIZED, "metadata allocation");
    return true;
}

static bool is_alnum(char c) {
    return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9');
}

static bool valid_dns(const char *host, size_t len) {
    if (len == 0 || len > 253 || host[0] == '.' || host[len - 1] == '.') return false;
    size_t start = 0;
    for (size_t i = 0; i <= len; i++) {
        if (i < len && host[i] != '.') {
            if (!is_alnum(host[i]) && host[i] != '-') return false;
            continue;
        }
        size_t n = i - start;
        if (n == 0 || n > 63 || host[start] == '-' || host[i - 1] == '-') return false;
        start = i + 1;
    }
    return true;
}

bool omt_is_undiallable_ipv4(const uint8_t a[4]) {
    bool unspecified = !a[0] && !a[1] && !a[2] && !a[3];
    bool multicast = a[0] >= 224 && a[0] <= 239;
    bool broadcast = a[0] == 255 && a[1] == 255 && a[2] == 255 && a[3] == 255;
    return unspecified || multicast || broadcast;
}

bool omt_is_undiallable_ipv6(const uint8_t a[16]) {
    bool unspecified = true;
    for (int i = 0; i < 16; i++) unspecified &= a[i] == 0;
    bool multicast = a[0] == 0xff;
    uint16_t first = (uint16_t)((a[0] << 8) | a[1]);
    return unspecified || multicast || (first & 0xFFC0) == 0xFE80;
}

bool omt_parse_direct_target(const char *value, size_t len, omt_direct_target *out,
                             omt_proto_error *err) {
    if (len > OMT_TARGET_MAX_BYTES || len < 6 || memcmp(value, "omt://", 6) != 0)
        return fail(err, OMT_PROTO_INVALID, "OMT direct target");
    const char *auth = value + 6;
    size_t alen = len - 6;
    for (size_t i = 0; i < alen; i++) {
        unsigned char b = (unsigned char)auth[i];
        if (b == '/' || b == '?' || b == '#' || b == '@' || b <= 31 || b == 127)
            return fail(err, OMT_PROTO_INVALID, "OMT direct target");
    }
    const char *host, *port_text;
    size_t host_len, port_len;
    bool ipv6 = alen > 0 && auth[0] == '[';
    if (ipv6) {
        const char *rest = auth + 1;
        size_t rlen = alen - 1;
        const char *close = NULL;
        for (size_t i = 0; i + 1 < rlen; i++)
            if (rest[i] == ']' && rest[i + 1] == ':') {
                close = rest + i;
                break;
            }
        if (!close) return fail(err, OMT_PROTO_INVALID, "IPv6 target");
        host = rest;
        host_len = (size_t)(close - rest);
        port_text = close + 2;
        port_len = rlen - host_len - 2;
        if (memchr(port_text, ':', port_len)) return fail(err, OMT_PROTO_INVALID, "IPv6 target");
    } else {
        const char *colon = NULL;
        for (size_t i = alen; i > 0; i--)
            if (auth[i - 1] == ':') {
                colon = auth + i - 1;
                break;
            }
        if (!colon) return fail(err, OMT_PROTO_INVALID, "OMT target port");
        host = auth;
        host_len = (size_t)(colon - auth);
        port_text = colon + 1;
        port_len = alen - host_len - 1;
        if (memchr(host, ':', host_len)) return fail(err, OMT_PROTO_INVALID, "IPv6 brackets");
    }
    uint64_t port;
    if (!omt_parse_u64(port_text, port_len, 65535, &port))
        return fail(err, OMT_PROTO_INVALID, "OMT target port");
    if (port == 0) return fail(err, OMT_PROTO_INVALID, "OMT target port");
    if (ipv6) {
        uint8_t a[16];
        if (memchr(host, '%', host_len) || !omt_parse_ipv6(host, host_len, a) ||
            omt_is_undiallable_ipv6(a))
            return fail(err, OMT_PROTO_INVALID, "IPv6 target");
    } else {
        uint8_t a[4];
        bool v4 = omt_parse_ipv4(host, host_len, a);
        if (host_len == 0 || host_len >= OMT_HOST_MAX_BYTES || (!v4 && !valid_dns(host, host_len)))
            return fail(err, OMT_PROTO_INVALID, "OMT target host");
        if (v4 && omt_is_undiallable_ipv4(a))
            return fail(err, OMT_PROTO_INVALID, "OMT target host");
    }
    if (host_len >= sizeof(out->host)) return fail(err, OMT_PROTO_INVALID, "OMT target host");
    memcpy(out->host, host, host_len);
    out->host[host_len] = 0;
    out->port = (uint16_t)port;
    return true;
}

const omt_codepoint_range omt_forbidden_name_ranges[] = {
    {0x0, 0x1f},        {0x7f, 0x9f},       {0xad, 0xad},       {0x600, 0x605},
    {0x61c, 0x61c},     {0x6dd, 0x6dd},     {0x70f, 0x70f},     {0x890, 0x891},
    {0x8e2, 0x8e2},     {0x180e, 0x180e},   {0x200b, 0x200f},   {0x2028, 0x202e},
    {0x2060, 0x2064},   {0x2066, 0x206f},   {0xd800, 0xdfff},   {0xfeff, 0xfeff},
    {0xfff9, 0xfffb},   {0x110bd, 0x110bd}, {0x110cd, 0x110cd}, {0x13430, 0x1343f},
    {0x1bca0, 0x1bca3}, {0x1d173, 0x1d17a}, {0xe0001, 0xe0001}, {0xe0020, 0xe007f},
};
const size_t omt_forbidden_name_range_count = OMT_ARRAY_LEN(omt_forbidden_name_ranges);

bool omt_is_forbidden_name_char(uint32_t cp) {
    size_t lo = 0, hi = omt_forbidden_name_range_count;
    while (lo < hi) {
        size_t mid = lo + (hi - lo) / 2;
        if (cp < omt_forbidden_name_ranges[mid].low)
            hi = mid;
        else if (cp > omt_forbidden_name_ranges[mid].high)
            lo = mid + 1;
        else
            return true;
    }
    return false;
}

bool omt_is_valid_source_name(const char *value, size_t len) {
    if (len == 0 || len > OMT_SOURCE_NAME_MAX_BYTES || !omt_utf8_valid(value, len)) return false;
    if (!omt_nfc_is_normalized(value, len, OMT_SOURCE_NAME_MAX_BYTES)) return false;
    size_t i = 0;
    uint32_t first = 0, last = 0;
    while (i < len) {
        bool at_start = i == 0;
        uint32_t cp = omt_utf8_next(value, len, &i);
        if (omt_is_forbidden_name_char(cp)) return false;
        if (at_start) first = cp;
        last = cp;
    }
    return !omt_unicode_is_whitespace(first) && !omt_unicode_is_whitespace(last);
}

bool omt_is_valid_target(const char *value, size_t len) {
    if (len >= 6 && memcmp(value, "omt://", 6) == 0) {
        omt_direct_target t;
        return omt_parse_direct_target(value, len, &t, NULL);
    }
    return omt_is_valid_source_name(value, len);
}
