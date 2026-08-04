/*
 * Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "omt/omt_wire.h"

#include <arpa/inet.h>
#include <ctype.h>
#include <limits.h>
#include <stdio.h>
#include <string.h>

static uint16_t read_u16(const uint8_t *data)
{
    return (uint16_t)((uint16_t)data[0] | ((uint16_t)data[1] << 8u));
}

static size_t bounded_strlen(const char *value, size_t maximum)
{
    size_t length = 0u;
    while (length < maximum && value[length] != '\0') {
        ++length;
    }
    return length;
}

static uint32_t read_u32(const uint8_t *data)
{
    return (uint32_t)data[0] |
           ((uint32_t)data[1] << 8u) |
           ((uint32_t)data[2] << 16u) |
           ((uint32_t)data[3] << 24u);
}

static int32_t read_i32(const uint8_t *data)
{
    uint32_t value = read_u32(data);
    int32_t result = 0;
    memcpy(&result, &value, sizeof(result));
    return result;
}

static int64_t read_i64(const uint8_t *data)
{
    uint64_t value = (uint64_t)read_u32(data) |
                     ((uint64_t)read_u32(data + 4u) << 32u);
    int64_t result = 0;
    memcpy(&result, &value, sizeof(result));
    return result;
}

static float read_float(const uint8_t *data)
{
    uint32_t bits = read_u32(data);
    float result = 0.0F;
    memcpy(&result, &bits, sizeof(result));
    return result;
}

static void write_u16(uint8_t *data, uint16_t value)
{
    data[0] = (uint8_t)value;
    data[1] = (uint8_t)(value >> 8u);
}

static void write_u32(uint8_t *data, uint32_t value)
{
    data[0] = (uint8_t)value;
    data[1] = (uint8_t)(value >> 8u);
    data[2] = (uint8_t)(value >> 16u);
    data[3] = (uint8_t)(value >> 24u);
}

static void write_i64(uint8_t *data, int64_t value)
{
    uint64_t bits = 0;
    memcpy(&bits, &value, sizeof(bits));
    write_u32(data, (uint32_t)bits);
    write_u32(data + 4u, (uint32_t)(bits >> 32u));
}

static bool fail(char *error, size_t error_length, const char *message)
{
    if (error != NULL && error_length > 0u) {
        (void)snprintf(error, error_length, "%s", message);
    }
    return false;
}

static size_t extended_length(omt_frame_type type)
{
    if (type == OMT_FRAME_VIDEO) {
        return OMT_WIRE_VIDEO_HEADER_SIZE;
    }
    if (type == OMT_FRAME_AUDIO) {
        return OMT_WIRE_AUDIO_HEADER_SIZE;
    }
    return 0u;
}

static size_t maximum_payload(omt_frame_type type)
{
    if (type == OMT_FRAME_VIDEO) {
        return OMT_WIRE_VIDEO_MAX_SIZE;
    }
    if (type == OMT_FRAME_AUDIO) {
        return OMT_WIRE_AUDIO_MAX_SIZE;
    }
    return OMT_WIRE_METADATA_MAX_SIZE;
}

bool omt_wire_parse_header(
    const uint8_t *data,
    size_t length,
    omt_frame_header *header,
    char *error,
    size_t error_length)
{
    if (data == NULL || header == NULL || length < OMT_WIRE_HEADER_SIZE) {
        return fail(error, error_length, "truncated OMT frame header");
    }
    if (data[0] != 1u) {
        return fail(error, error_length, "unsupported OMT frame version");
    }
    omt_frame_type type = (omt_frame_type)data[1];
    if (type != OMT_FRAME_METADATA && type != OMT_FRAME_VIDEO && type != OMT_FRAME_AUDIO) {
        return fail(error, error_length, "unsupported OMT frame type");
    }

    uint32_t data_length = read_u32(data + 12u);
    size_t extension = extended_length(type);
    if ((size_t)data_length < extension) {
        return fail(error, error_length, "OMT frame is shorter than its extended header");
    }
    size_t payload = (size_t)data_length - extension;
    if (payload > maximum_payload(type)) {
        return fail(error, error_length, "OMT frame payload exceeds its limit");
    }
    uint16_t metadata_length = read_u16(data + 10u);
    if ((size_t)metadata_length > payload) {
        return fail(error, error_length, "OMT metadata length exceeds the payload");
    }

    header->version = data[0];
    header->type = type;
    header->timestamp = read_i64(data + 2u);
    header->metadata_length = metadata_length;
    header->data_length = data_length;
    if (error != NULL && error_length > 0u) {
        error[0] = '\0';
    }
    return true;
}

bool omt_wire_parse_video_header(
    const omt_frame_header *frame,
    const uint8_t *data,
    size_t length,
    omt_video_header *video,
    char *error,
    size_t error_length)
{
    if (frame == NULL || data == NULL || video == NULL || frame->type != OMT_FRAME_VIDEO ||
        length < OMT_WIRE_VIDEO_HEADER_SIZE) {
        return fail(error, error_length, "truncated OMT video header");
    }
    video->codec = (omt_codec)read_i32(data);
    video->width = read_i32(data + 4u);
    video->height = read_i32(data + 8u);
    video->frame_rate_n = read_i32(data + 12u);
    video->frame_rate_d = read_i32(data + 16u);
    video->aspect_ratio = read_float(data + 20u);
    video->flags = read_u32(data + 24u);
    video->color_space = read_i32(data + 28u);
    if (video->width < 16 || video->height < 16 || video->width > 1920 || video->height > 1080 ||
        video->frame_rate_n <= 0 || video->frame_rate_d <= 0) {
        return fail(error, error_length, "unsupported OMT video dimensions or frame rate");
    }
    double rate = (double)video->frame_rate_n / (double)video->frame_rate_d;
    if (rate <= 0.0 || rate > 60.0) {
        return fail(error, error_length, "unsupported OMT video frame rate");
    }
    if (video->codec != OMT_CODEC_VMX1) {
        return fail(error, error_length, "unsupported OMT video codec");
    }
    if (!(video->aspect_ratio >= 0.0F && video->aspect_ratio <= 10.0F) ||
        (video->color_space != 0 && video->color_space != 601 && video->color_space != 709) ||
        (video->flags & ~31u) != 0u) {
        return fail(error, error_length, "unsupported OMT video properties");
    }
    return true;
}

bool omt_wire_parse_audio_header(
    const omt_frame_header *frame,
    const uint8_t *data,
    size_t length,
    omt_audio_header *audio,
    char *error,
    size_t error_length)
{
    if (frame == NULL || data == NULL || audio == NULL || frame->type != OMT_FRAME_AUDIO ||
        length < OMT_WIRE_AUDIO_HEADER_SIZE) {
        return fail(error, error_length, "truncated OMT audio header");
    }
    audio->codec = (omt_codec)read_i32(data);
    audio->sample_rate = read_i32(data + 4u);
    audio->samples_per_channel = read_i32(data + 8u);
    audio->channels = read_i32(data + 12u);
    audio->active_channels = read_u32(data + 16u);
    if (audio->codec != OMT_CODEC_FPA1 || audio->sample_rate < 8000 || audio->sample_rate > 192000 ||
        audio->channels < 1 || audio->channels > 32 || audio->samples_per_channel < 1) {
        return fail(error, error_length, "unsupported OMT audio format");
    }
    size_t channels = (size_t)audio->channels;
    size_t samples = (size_t)audio->samples_per_channel;
    if (samples > OMT_WIRE_AUDIO_MAX_SIZE / sizeof(float) / channels) {
        return fail(error, error_length, "OMT decoded audio exceeds its limit");
    }
    unsigned int active_count = 0u;
    uint32_t active = audio->active_channels;
    uint32_t allowed_channels = audio->channels == 32
                                    ? UINT32_MAX
                                    : ((uint32_t)1u << (unsigned int)audio->channels) - 1u;
    if ((active & ~allowed_channels) != 0u) {
        return fail(error, error_length, "OMT audio names an out-of-range active channel");
    }
    while (active != 0u) {
        active_count += active & 1u;
        active >>= 1u;
    }
    size_t compressed = (size_t)active_count * samples * sizeof(float);
    size_t payload = (size_t)frame->data_length - OMT_WIRE_AUDIO_HEADER_SIZE;
    if (compressed + (size_t)frame->metadata_length != payload) {
        return fail(error, error_length, "invalid OMT planar audio payload length");
    }
    return true;
}

bool omt_wire_build_metadata(
    const char *xml,
    int64_t timestamp,
    uint8_t *output,
    size_t capacity,
    size_t *written)
{
    if (xml == NULL || output == NULL || written == NULL) {
        return false;
    }
    size_t xml_length = bounded_strlen(xml, OMT_WIRE_METADATA_MAX_SIZE);
    if (xml_length == 0u || xml_length >= OMT_WIRE_METADATA_MAX_SIZE ||
        capacity < OMT_WIRE_HEADER_SIZE + xml_length) {
        return false;
    }
    output[0] = 1u;
    output[1] = (uint8_t)OMT_FRAME_METADATA;
    write_i64(output + 2u, timestamp);
    write_u16(output + 10u, 0u);
    write_u32(output + 12u, (uint32_t)xml_length);
    memcpy(output + OMT_WIRE_HEADER_SIZE, xml, xml_length);
    *written = OMT_WIRE_HEADER_SIZE + xml_length;
    return true;
}

static bool valid_dns_name(const char *host, size_t length)
{
    if (length == 0u || length > 253u || host[0] == '.' || host[length - 1u] == '.') {
        return false;
    }
    size_t label = 0u;
    for (size_t index = 0u; index < length; ++index) {
        unsigned char ch = (unsigned char)host[index];
        if (ch == '.') {
            if (label == 0u || label > 63u || host[index - 1u] == '-') {
                return false;
            }
            label = 0u;
            continue;
        }
        if (!((ch >= 'a' && ch <= 'z') || (ch >= 'A' && ch <= 'Z') ||
              (ch >= '0' && ch <= '9') || ch == '-')) {
            return false;
        }
        if (label == 0u && ch == '-') {
            return false;
        }
        ++label;
    }
    return label > 0u && label <= 63u && host[length - 1u] != '-';
}

bool omt_parse_direct_target(const char *value, omt_direct_target *target)
{
    static const char prefix[] = "omt://";
    if (value == NULL || target == NULL) {
        return false;
    }
    size_t length = bounded_strlen(value, OMT_TARGET_MAX_BYTES + 1u);
    if (length <= sizeof(prefix) - 1u || length > OMT_TARGET_MAX_BYTES ||
        strncmp(value, prefix, sizeof(prefix) - 1u) != 0) {
        return false;
    }
    const char *authority = value + sizeof(prefix) - 1u;
    if (strpbrk(authority, "/?#@") != NULL) {
        return false;
    }

    const char *host_start = authority;
    const char *host_end = NULL;
    const char *port_start = NULL;
    bool ipv6 = false;
    if (*host_start == '[') {
        ipv6 = true;
        ++host_start;
        host_end = strchr(host_start, ']');
        if (host_end == NULL || host_end[1] != ':') {
            return false;
        }
        port_start = host_end + 2;
    } else {
        const char *colon = strrchr(host_start, ':');
        if (colon == NULL || strchr(host_start, ':') != colon) {
            return false;
        }
        host_end = colon;
        port_start = colon + 1;
    }
    size_t host_length = (size_t)(host_end - host_start);
    if (host_length == 0u || host_length >= sizeof(target->host) || *port_start == '\0') {
        return false;
    }
    unsigned long port = 0ul;
    for (const char *cursor = port_start; *cursor != '\0'; ++cursor) {
        if (!isdigit((unsigned char)*cursor)) {
            return false;
        }
        port = port * 10ul + (unsigned long)(*cursor - '0');
        if (port > 65535ul) {
            return false;
        }
    }
    if (port == 0ul) {
        return false;
    }

    char host[sizeof(target->host)];
    memcpy(host, host_start, host_length);
    host[host_length] = '\0';
    if (ipv6) {
        char *zone = strstr(host, "%25");
        if (zone != NULL) {
            const char *zone_name = zone + 3;
            if (*zone_name == '\0' || strchr(zone_name, '%') != NULL) {
                return false;
            }
            for (const char *cursor = zone_name; *cursor != '\0'; ++cursor) {
                unsigned char ch = (unsigned char)*cursor;
                if (!((ch >= 'a' && ch <= 'z') || (ch >= 'A' && ch <= 'Z') ||
                      (ch >= '0' && ch <= '9') || ch == '_' || ch == '-' || ch == '.')) {
                    return false;
                }
            }
            *zone = '\0';
        } else if (strchr(host, '%') != NULL) {
            return false;
        }
        struct in6_addr address6;
        if (inet_pton(AF_INET6, host, &address6) != 1) {
            return false;
        }
        if (zone != NULL) {
            const char *zone_name = zone + 3;
            size_t zone_length = strlen(zone_name);
            *zone = '%';
            memmove(zone + 1, zone_name, zone_length + 1u);
            host_length -= 2u;
        }
    } else {
        struct in_addr address4;
        if (inet_pton(AF_INET, host, &address4) != 1 && !valid_dns_name(host, host_length)) {
            return false;
        }
    }
    memcpy(target->host, host, host_length + 1u);
    target->port = (uint16_t)port;
    target->ipv6_literal = ipv6;
    return true;
}

static bool valid_utf8_scalar(const unsigned char **cursor, const unsigned char *end, uint32_t *scalar)
{
    unsigned char first = **cursor;
    if (first < 0x80u) {
        *scalar = first;
        ++(*cursor);
        return true;
    }
    unsigned int count = 0u;
    uint32_t value = 0u;
    uint32_t minimum = 0u;
    if ((first & 0xe0u) == 0xc0u) {
        count = 1u; value = first & 0x1fu; minimum = 0x80u;
    } else if ((first & 0xf0u) == 0xe0u) {
        count = 2u; value = first & 0x0fu; minimum = 0x800u;
    } else if ((first & 0xf8u) == 0xf0u) {
        count = 3u; value = first & 0x07u; minimum = 0x10000u;
    } else {
        return false;
    }
    if ((size_t)(end - *cursor) <= count) {
        return false;
    }
    ++(*cursor);
    for (unsigned int index = 0u; index < count; ++index) {
        unsigned char next = **cursor;
        if ((next & 0xc0u) != 0x80u) {
            return false;
        }
        value = (value << 6u) | (uint32_t)(next & 0x3fu);
        ++(*cursor);
    }
    if (value < minimum || value > 0x10ffffu || (value >= 0xd800u && value <= 0xdfffu)) {
        return false;
    }
    *scalar = value;
    return true;
}

static bool unicode_strip_space(uint32_t scalar)
{
    return (scalar >= 0x0009u && scalar <= 0x000du) ||
           (scalar >= 0x001cu && scalar <= 0x0020u) || scalar == 0x0085u ||
           scalar == 0x00a0u || scalar == 0x1680u ||
           (scalar >= 0x2000u && scalar <= 0x200au) || scalar == 0x2028u ||
           scalar == 0x2029u || scalar == 0x202fu || scalar == 0x205fu || scalar == 0x3000u;
}

bool omt_is_valid_source_name_utf8(const char *value)
{
    if (value == NULL) {
        return false;
    }
    size_t length = bounded_strlen(value, OMT_SOURCE_NAME_MAX_BYTES + 1u);
    if (length == 0u || length > OMT_SOURCE_NAME_MAX_BYTES || value[0] == ' ' ||
        value[length - 1u] == ' ') {
        return false;
    }
    const unsigned char *cursor = (const unsigned char *)value;
    const unsigned char *end = cursor + length;
    uint32_t first_scalar = 0u;
    uint32_t last_scalar = 0u;
    while (cursor < end) {
        uint32_t scalar = 0u;
        if (!valid_utf8_scalar(&cursor, end, &scalar)) {
            return false;
        }
        if (first_scalar == 0u) {
            first_scalar = scalar;
        }
        last_scalar = scalar;
        if (scalar <= 0x1fu || scalar == 0x7fu || scalar == 0x2028u || scalar == 0x2029u ||
            (scalar >= 0x0300u && scalar <= 0x036fu) ||
            (scalar >= 0x1ab0u && scalar <= 0x1affu) ||
            (scalar >= 0x1dc0u && scalar <= 0x1dffu) ||
            (scalar >= 0x20d0u && scalar <= 0x20ffu) ||
            (scalar >= 0xfe20u && scalar <= 0xfe2fu) ||
            (scalar >= 0x200bu && scalar <= 0x200fu) ||
            (scalar >= 0x202au && scalar <= 0x202eu) ||
            (scalar >= 0x2060u && scalar <= 0x206fu) || scalar == 0xfeffu) {
            return false;
        }
    }
    return !unicode_strip_space(first_scalar) && !unicode_strip_space(last_scalar);
}
