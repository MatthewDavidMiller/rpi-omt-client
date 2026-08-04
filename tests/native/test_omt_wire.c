/* Copyright (c) 2026 Matthew David Miller; SPDX-License-Identifier: MIT */
#include "omt/omt_wire.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static void require_true(bool value, const char *message)
{
    if (!value) {
        (void)fprintf(stderr, "FAIL: %s\n", message);
        exit(1);
    }
}

static void require_false(bool value, const char *message)
{
    require_true(!value, message);
}

static void write_u32(unsigned char *data, unsigned int value)
{
    data[0] = (unsigned char)value;
    data[1] = (unsigned char)(value >> 8u);
    data[2] = (unsigned char)(value >> 16u);
    data[3] = (unsigned char)(value >> 24u);
}

static void target_contract(void)
{
    omt_direct_target target;
    require_true(omt_parse_direct_target("omt://camera:6400", &target), "DNS target is valid");
    require_true(strcmp(target.host, "camera") == 0 && target.port == 6400u, "DNS target is parsed");
    require_true(omt_parse_direct_target("omt://[2001:db8::1]:1", &target), "IPv6 target is valid");
    require_true(omt_parse_direct_target("omt://192.0.2.1:65535", &target), "IPv4 target is valid");
    require_true(omt_parse_direct_target("omt://CAMERA-1:0001", &target), "leading-zero port is valid");

    const char *invalid[] = {
        "", "camera", "omt://", "omt://camera", "omt://camera:0", "omt://camera:65536",
        "omt://user@camera:1", "omt://camera:1/path", "omt://camera:1?x", "omt://camera:1#x",
        "omt://host_name:1", "omt://-camera:1", "omt://camera-:1", "omt://[192.0.2.1]:1",
        "omt://[]:1", "omt://[::1]1", "omt://[::1]:"
    };
    for (size_t index = 0u; index < sizeof(invalid) / sizeof(invalid[0]); ++index) {
        require_false(omt_parse_direct_target(invalid[index], &target), invalid[index]);
    }

    require_true(omt_is_valid_source_name_utf8("Camera"), "ASCII source name is valid");
    require_true(omt_is_valid_source_name_utf8("Camera \xf0\x9f\x98\x80"), "UTF-8 source name is valid");
    require_false(omt_is_valid_source_name_utf8(" Camera"), "leading space is invalid");
    require_false(omt_is_valid_source_name_utf8("Camera\n"), "control is invalid");
    require_false(omt_is_valid_source_name_utf8("\xf0\x28\x8c\x28"), "malformed UTF-8 is invalid");
    require_false(omt_is_valid_source_name_utf8("Cafe\xcc\x81"), "decomposed source name is invalid");
}

static void frame_contract(void)
{
    unsigned char frame[OMT_WIRE_HEADER_SIZE + OMT_WIRE_VIDEO_HEADER_SIZE] = {0};
    frame[0] = 1u;
    frame[1] = (unsigned char)OMT_FRAME_VIDEO;
    write_u32(frame + 12u, OMT_WIRE_VIDEO_HEADER_SIZE);
    write_u32(frame + OMT_WIRE_HEADER_SIZE, (unsigned int)OMT_CODEC_VMX1);
    write_u32(frame + OMT_WIRE_HEADER_SIZE + 4u, 1920u);
    write_u32(frame + OMT_WIRE_HEADER_SIZE + 8u, 1080u);
    write_u32(frame + OMT_WIRE_HEADER_SIZE + 12u, 60000u);
    write_u32(frame + OMT_WIRE_HEADER_SIZE + 16u, 1000u);
    write_u32(frame + OMT_WIRE_HEADER_SIZE + 28u, 709u);

    omt_frame_header header;
    omt_video_header video;
    char error[128];
    require_true(omt_wire_parse_header(frame, sizeof(frame), &header, error, sizeof(error)), "frame header parses");
    require_true(
        omt_wire_parse_video_header(
            &header, frame + OMT_WIRE_HEADER_SIZE, OMT_WIRE_VIDEO_HEADER_SIZE,
            &video, error, sizeof(error)),
        "video header parses");
    require_true(video.width == 1920 && video.height == 1080 && video.frame_rate_n == 60000,
                 "video fields are little-endian");

    write_u32(frame + OMT_WIRE_HEADER_SIZE + 24u, 32u);
    require_false(
        omt_wire_parse_video_header(
            &header, frame + OMT_WIRE_HEADER_SIZE, OMT_WIRE_VIDEO_HEADER_SIZE,
            &video, error, sizeof(error)),
        "unknown video flags are rejected");
    write_u32(frame + OMT_WIRE_HEADER_SIZE + 24u, 0u);
    write_u32(frame + OMT_WIRE_HEADER_SIZE + 20u, 0x7fc00000u);
    require_false(
        omt_wire_parse_video_header(
            &header, frame + OMT_WIRE_HEADER_SIZE, OMT_WIRE_VIDEO_HEADER_SIZE,
            &video, error, sizeof(error)),
        "non-finite video aspect ratio is rejected");

    frame[0] = 2u;
    require_false(omt_wire_parse_header(frame, sizeof(frame), &header, error, sizeof(error)),
                  "unknown wire version is rejected");
    frame[0] = 1u;
    write_u32(frame + 12u, OMT_WIRE_VIDEO_MAX_SIZE + OMT_WIRE_VIDEO_HEADER_SIZE + 1u);
    require_false(omt_wire_parse_header(frame, sizeof(frame), &header, error, sizeof(error)),
                  "oversize video frame is rejected");
}

static void audio_contract(void)
{
    unsigned char frame[OMT_WIRE_HEADER_SIZE + OMT_WIRE_AUDIO_HEADER_SIZE + 8u] = {0};
    frame[0] = 1u;
    frame[1] = (unsigned char)OMT_FRAME_AUDIO;
    write_u32(frame + 12u, OMT_WIRE_AUDIO_HEADER_SIZE + 8u);
    write_u32(frame + OMT_WIRE_HEADER_SIZE, (unsigned int)OMT_CODEC_FPA1);
    write_u32(frame + OMT_WIRE_HEADER_SIZE + 4u, 48000u);
    write_u32(frame + OMT_WIRE_HEADER_SIZE + 8u, 2u);
    write_u32(frame + OMT_WIRE_HEADER_SIZE + 12u, 2u);
    write_u32(frame + OMT_WIRE_HEADER_SIZE + 16u, 1u);

    omt_frame_header header;
    omt_audio_header audio;
    char error[128];
    require_true(omt_wire_parse_header(frame, sizeof(frame), &header, error, sizeof(error)),
                 "audio frame header parses");
    require_true(
        omt_wire_parse_audio_header(
            &header, frame + OMT_WIRE_HEADER_SIZE, OMT_WIRE_AUDIO_HEADER_SIZE,
            &audio, error, sizeof(error)),
        "bounded planar audio payload parses");
    write_u32(frame + OMT_WIRE_HEADER_SIZE + 16u, 4u);
    require_false(
        omt_wire_parse_audio_header(
            &header, frame + OMT_WIRE_HEADER_SIZE, OMT_WIRE_AUDIO_HEADER_SIZE,
            &audio, error, sizeof(error)),
        "out-of-range active audio channels are rejected");
    write_u32(frame + OMT_WIRE_HEADER_SIZE + 16u, 1u);
    header.data_length -= 4u;
    require_false(
        omt_wire_parse_audio_header(
            &header, frame + OMT_WIRE_HEADER_SIZE, OMT_WIRE_AUDIO_HEADER_SIZE,
            &audio, error, sizeof(error)),
        "mismatched planar audio payload length is rejected");
}

static void metadata_contract(void)
{
    static const char xml[] = "<OMTSubscribe Video=\"true\" />";
    unsigned char frame[128];
    size_t written = 0u;
    require_true(omt_wire_build_metadata(xml, 42, frame, sizeof(frame), &written), "metadata is encoded");
    omt_frame_header header;
    char error[128];
    require_true(omt_wire_parse_header(frame, written, &header, error, sizeof(error)), "metadata header parses");
    require_true(header.type == OMT_FRAME_METADATA && header.timestamp == 42 &&
                     header.metadata_length == 0u &&
                     header.data_length == sizeof(xml) - 1u &&
                     written == OMT_WIRE_HEADER_SIZE + sizeof(xml) - 1u,
                 "metadata fields match the wire contract");
    require_true(memcmp(frame + OMT_WIRE_HEADER_SIZE, xml, sizeof(xml) - 1u) == 0,
                 "metadata body matches the exact subscription command");
}

int main(void)
{
    target_contract();
    frame_contract();
    audio_contract();
    metadata_contract();
    (void)puts("native OMT wire contracts passed");
    return 0;
}
