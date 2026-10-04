/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Port of the omt-protocol crate's tests, plus the shared schema vectors.
 */
#include <stdlib.h>

#include "common/fsio.h"
#include "common/json.h"
#include "protocol/omt.h"
#include "test.h"

static bool target_ok(const char *s) {
    omt_direct_target t;
    return omt_parse_direct_target(s, strlen(s), &t, NULL);
}

static bool name_ok(const char *s) { return omt_is_valid_source_name(s, strlen(s)); }

static void target_contract(void) {
    omt_direct_target t;
    CHECK(omt_parse_direct_target("omt://camera:6400", 17, &t, NULL));
    CHECK_INT(t.port, 6400);
    CHECK_STR(t.host, "camera");
    CHECK(!target_ok("omt://[fe80::1%25eth0]:6400"));
    CHECK(!target_ok("omt://[fe80::1]:6400"));
    CHECK(!target_ok("omt://0.0.0.0:6400"));
    CHECK(!target_ok("omt://255.255.255.255:1"));
    const char *invalid[] = {"",
                             "camera",
                             "omt://camera:0",
                             "omt://user@camera:1",
                             "omt://host_name:1",
                             "omt://[192.0.2.1]:1"};
    for (size_t i = 0; i < OMT_ARRAY_LEN(invalid); i++)
        CHECK_MSG(!target_ok(invalid[i]), "%s", invalid[i]);
    CHECK(name_ok("Camera \xf0\x9f\x98\x80"));
    CHECK(!name_ok("Cafe\xcc\x81"));
    CHECK(target_ok("omt://[2001:db8::1]:6400"));
    CHECK(target_ok("omt://192.0.2.1:6400"));
}

static void metadata_round_trip(void) {
    omt_buf b;
    omt_buf_init(&b, 1024);
    CHECK(omt_build_metadata("<x/>", 4, 42, &b, NULL));
    omt_frame_header h;
    CHECK(omt_parse_frame_header(b.data, b.len, &h, NULL));
    CHECK_INT(h.timestamp, 42);
    CHECK_INT(h.frame_type, OMT_FRAME_METADATA);
    omt_buf_free(&b);
}

static void video_bytes(const omt_video_header *v, uint8_t out[32]) {
    int32_t fields[8] = {v->codec,        v->width, v->height,         v->frame_rate_n,
                         v->frame_rate_d, 0,        (int32_t)v->flags, v->color_space};
    for (int i = 0; i < 8; i++) omt_put_le32(out + 4 * i, (uint32_t)fields[i]);
    uint32_t bits;
    memcpy(&bits, &v->aspect_ratio, 4);
    omt_put_le32(out + 20, bits);
}

static omt_video_header supported(void) {
    omt_video_header v = {OMT_CODEC_VMX1, 1920, 1080, 60, 1, 16.0f / 9.0f, 0, 709};
    return v;
}

static void video_headers_accept_only_the_appliance_format(void) {
    omt_frame_header h = {OMT_FRAME_VIDEO, 0, 0, OMT_VIDEO_HEADER_SIZE};
    uint8_t bytes[32];
    omt_video_header v = supported(), out;
    video_bytes(&v, bytes);
    CHECK(omt_parse_video_header(&h, bytes, 32, &out, NULL));
    CHECK(out.width == 1920 && out.color_space == 709);
    v.flags = 1;
    video_bytes(&v, bytes);
    CHECK(omt_parse_video_header(&h, bytes, 32, &out, NULL));

    omt_video_header bad[8];
    for (int i = 0; i < 8; i++) bad[i] = supported();
    bad[0].width = 1922;
    bad[1].height = 1082;
    bad[2].frame_rate_n = 120;
    bad[3].frame_rate_d = 0;
    bad[4].codec = OMT_CODEC_FPA1;
    bad[5].color_space = 2020;
    bad[6].flags = 32;
    bad[7].aspect_ratio = __builtin_nanf("");
    for (int i = 0; i < 8; i++) {
        video_bytes(&bad[i], bytes);
        CHECK_MSG(!omt_parse_video_header(&h, bytes, 32, &out, NULL), "case %d", i);
    }
    omt_frame_header audio = {OMT_FRAME_AUDIO, 0, 0, OMT_VIDEO_HEADER_SIZE};
    v = supported();
    video_bytes(&v, bytes);
    CHECK(!omt_parse_video_header(&audio, bytes, 32, &out, NULL));
    CHECK(!omt_parse_video_header(&h, bytes, 31, &out, NULL));
}

static void audio_bytes(int32_t codec, int32_t rate, int32_t samples, int32_t channels,
                        uint32_t active, uint8_t out[24]) {
    memset(out, 0, 24);
    omt_put_le32(out, (uint32_t)codec);
    omt_put_le32(out + 4, (uint32_t)rate);
    omt_put_le32(out + 8, (uint32_t)samples);
    omt_put_le32(out + 12, (uint32_t)channels);
    omt_put_le32(out + 16, active);
}

static void audio_headers_bind_the_payload_to_the_active_channels(void) {
    uint32_t compressed = 480 * 2 * 4;
    omt_frame_header h = {OMT_FRAME_AUDIO, 0, 0, OMT_AUDIO_HEADER_SIZE + compressed};
    uint8_t b[24];
    omt_audio_header a;
    audio_bytes(OMT_CODEC_FPA1, 48000, 480, 2, 3, b);
    CHECK(omt_parse_audio_header(&h, b, 24, &a, NULL) && a.channels == 2);
    audio_bytes(OMT_CODEC_FPA1, 48000, 480, 2, 1, b);
    CHECK(!omt_parse_audio_header(&h, b, 24, &a, NULL));
    omt_frame_header halved = {OMT_FRAME_AUDIO, 0, 0, OMT_AUDIO_HEADER_SIZE + compressed / 2};
    CHECK(omt_parse_audio_header(&halved, b, 24, &a, NULL));
    audio_bytes(OMT_CODEC_FPA1, 48000, 480, 2, 3, b);
    omt_frame_header meta = {OMT_FRAME_AUDIO, 0, 16, OMT_AUDIO_HEADER_SIZE + compressed + 16};
    CHECK(omt_parse_audio_header(&meta, b, 24, &a, NULL));
    audio_bytes(OMT_CODEC_FPA1, 48000, 480, 2, 7, b);
    CHECK(!omt_parse_audio_header(&h, b, 24, &a, NULL));
    omt_frame_header full = {OMT_FRAME_AUDIO, 0, 0, OMT_AUDIO_HEADER_SIZE + 480 * 32 * 4};
    audio_bytes(OMT_CODEC_FPA1, 48000, 480, 32, UINT32_MAX, b);
    CHECK(omt_parse_audio_header(&full, b, 24, &a, NULL));
    struct {
        int32_t codec, rate, samples, channels;
        uint32_t active;
    } bad[] = {{OMT_CODEC_VMX1, 48000, 480, 2, 3},      {OMT_CODEC_FPA1, 4000, 480, 2, 3},
               {OMT_CODEC_FPA1, 200000, 480, 2, 3},     {OMT_CODEC_FPA1, 48000, 480, 33, 3},
               {OMT_CODEC_FPA1, 48000, 480, 0, 0},      {OMT_CODEC_FPA1, 48000, 0, 2, 3},
               {OMT_CODEC_FPA1, 48000, INT32_MAX, 2, 3}};
    for (size_t i = 0; i < OMT_ARRAY_LEN(bad); i++) {
        audio_bytes(bad[i].codec, bad[i].rate, bad[i].samples, bad[i].channels, bad[i].active, b);
        CHECK_MSG(!omt_parse_audio_header(&h, b, 24, &a, NULL), "case %zu", i);
    }
}

static void frame_headers_bound_every_payload(void) {
    omt_buf b;
    omt_buf_init(&b, 1024);
    CHECK(omt_build_metadata("<x/>", 4, 0, &b, NULL));
    omt_frame_header h;
    omt_proto_error e;
    b.data[0] = 2;
    CHECK(!omt_parse_frame_header(b.data, b.len, &h, &e));
    CHECK_STR(e.detail, "OMT frame version");
    CHECK_INT(e.kind, OMT_PROTO_UNSUPPORTED);
    b.data[0] = 1;
    b.data[1] = 3;
    CHECK(!omt_parse_frame_header(b.data, b.len, &h, &e));
    CHECK_STR(e.detail, "OMT frame type");
    CHECK(!omt_parse_frame_header(b.data, OMT_HEADER_SIZE - 1, &h, &e));
    CHECK_INT(e.kind, OMT_PROTO_TRUNCATED);
    omt_buf_free(&b);

    uint8_t header[16] = {1, OMT_FRAME_VIDEO};
    omt_put_le32(header + 12, OMT_VIDEO_HEADER_SIZE - 1);
    CHECK(!omt_parse_frame_header(header, 16, &h, &e) && e.kind == OMT_PROTO_INVALID);
    uint32_t ceiling = OMT_VIDEO_MAX_SIZE + OMT_VIDEO_HEADER_SIZE;
    omt_put_le32(header + 12, ceiling);
    CHECK(omt_parse_frame_header(header, 16, &h, &e));
    omt_put_le32(header + 12, ceiling + 1);
    CHECK(!omt_parse_frame_header(header, 16, &h, &e));
    CHECK_STR(e.detail, "OMT frame payload");
    CHECK_INT(e.kind, OMT_PROTO_OVERSIZED);
    omt_put_le32(header + 12, OMT_VIDEO_HEADER_SIZE + 8);
    omt_put_le16(header + 10, 9);
    CHECK(!omt_parse_frame_header(header, 16, &h, &e) && e.kind == OMT_PROTO_INVALID);
    char text[128];
    omt_proto_error fe = {OMT_PROTO_UNSUPPORTED, "OMT frame version"};
    omt_proto_error_format(&fe, text, sizeof(text));
    CHECK_STR(text, "unsupported OMT frame version");
}

static void shared_validation_vectors(void) {
    omt_buf raw;
    omt_err err;
    CHECK_INT(omt_read_bounded("tests/schema/omt-target-vectors.json", 1 << 20, &raw, &err),
              OMT_READ_OK);
    omt_json_doc doc;
    omt_json *root = omt_json_parse(&doc, omt_buf_cstr(&raw), raw.len, 0);
    CHECK(root != NULL);
    if (!root) return;
    const omt_json *names = omt_json_get(root, "source_names");
    for (size_t i = 0; names && i < names->count; i++) {
        const omt_json *v = omt_json_get(names->items[i], "value");
        bool valid = omt_json_get(names->items[i], "valid")->boolean;
        CHECK_MSG(omt_is_valid_source_name(v->string, v->string_len) == valid, "source %zu %s", i,
                  v->string);
    }
    const omt_json *targets = omt_json_get(root, "direct_targets");
    for (size_t i = 0; targets && i < targets->count; i++) {
        const omt_json *v = omt_json_get(targets->items[i], "value");
        bool valid = omt_json_get(targets->items[i], "valid")->boolean;
        omt_direct_target t;
        CHECK_MSG(omt_parse_direct_target(v->string, v->string_len, &t, NULL) == valid,
                  "target %zu %s", i, v->string);
    }
    /* The compiled table has to be the published one, range for range. */
    const omt_json *ranges =
        omt_json_get(omt_json_get(root, "forbidden_name_codepoints"), "ranges");
    CHECK(ranges && ranges->count == omt_forbidden_name_range_count);
    for (size_t i = 0; ranges && i < ranges->count && i < omt_forbidden_name_range_count; i++) {
        unsigned long low = strtoul(ranges->items[i]->items[0]->string, NULL, 16);
        unsigned long high = strtoul(ranges->items[i]->items[1]->string, NULL, 16);
        CHECK(low == omt_forbidden_name_ranges[i].low && high == omt_forbidden_name_ranges[i].high);
        if (i) CHECK(omt_forbidden_name_ranges[i - 1].high < omt_forbidden_name_ranges[i].low);
    }
    for (size_t i = 0; i < omt_forbidden_name_range_count; i++) {
        uint32_t bounds[2] = {omt_forbidden_name_ranges[i].low, omt_forbidden_name_ranges[i].high};
        for (int k = 0; k < 2; k++) {
            CHECK(omt_is_forbidden_name_char(bounds[k]));
            if (bounds[k] >= 0xD800 && bounds[k] <= 0xDFFF) continue;
            omt_buf s;
            omt_buf_init(&s, 16);
            omt_buf_putc(&s, 'a');
            omt_utf8_put(&s, bounds[k]);
            omt_buf_putc(&s, 'b');
            CHECK_MSG(!omt_is_valid_source_name(omt_buf_cstr(&s), s.len), "U+%04X", bounds[k]);
            omt_buf_free(&s);
        }
    }
    omt_json_doc_free(&doc);
    omt_buf_free(&raw);
}

int main(void) {
    RUN(target_contract);
    RUN(metadata_round_trip);
    RUN(video_headers_accept_only_the_appliance_format);
    RUN(audio_headers_bind_the_payload_to_the_active_channels);
    RUN(frame_headers_bound_every_payload);
    RUN(shared_validation_vectors);
    return TEST_EXIT();
}
