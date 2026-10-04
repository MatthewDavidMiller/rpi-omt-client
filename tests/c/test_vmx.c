/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * VMX conformance: every committed stream must decode to exactly the bytes
 * the Open Media Transport reference decoder produced, pinned by digest in
 * tests/vectors/vmx/vectors.json. Also the ported unit tests of the bit
 * reader, the plane helpers, and the kernel differential checks.
 */
#include <stdlib.h>

#include "common/fsio.h"
#include "common/json.h"
#include "sha256.h"
#include "test.h"
#include "vmx/bitstream.h"
#include "vmx/internal.h"
#include "vmx/plane.h"
#include "vmx/vmx.h"

#define VECTOR_DIR "tests/vectors/vmx/"

typedef struct {
    char label[64];
    size_t width, height;
    int32_t color_space;
    char stream[128];
    char uyvy[65];
    char bgrx[65];
} vector;

static size_t load_vectors(vector *out, size_t max) {
    omt_buf raw;
    omt_err err;
    if (omt_read_bounded(VECTOR_DIR "vectors.json", 1 << 20, &raw, &err) != OMT_READ_OK) return 0;
    omt_json_doc doc;
    omt_json *root = omt_json_parse(&doc, omt_buf_cstr(&raw), raw.len, 0);
    size_t n = 0;
    const omt_json *list = omt_json_get(root, "vectors");
    for (size_t i = 0; list && i < list->count && n < max; i++, n++) {
        const omt_json *v = list->items[i];
        uint64_t w = 0, h = 0;
        int64_t cs = 0;
        (void)omt_json_as_u64(omt_json_get(v, "width"), &w);
        (void)omt_json_as_u64(omt_json_get(v, "height"), &h);
        (void)omt_json_as_i64(omt_json_get(v, "color_space"), &cs);
        out[n].width = (size_t)w;
        out[n].height = (size_t)h;
        out[n].color_space = (int32_t)cs;
        snprintf(out[n].label, sizeof(out[n].label), "%s",
                 omt_json_as_str(omt_json_get(v, "label")));
        snprintf(out[n].stream, sizeof(out[n].stream), "%s",
                 omt_json_as_str(omt_json_get(v, "stream")));
        snprintf(out[n].uyvy, 65, "%s", omt_json_as_str(omt_json_get(v, "uyvy_sha256")));
        snprintf(out[n].bgrx, 65, "%s", omt_json_as_str(omt_json_get(v, "bgrx_sha256")));
    }
    omt_json_doc_free(&doc);
    omt_buf_free(&raw);
    return n;
}

static bool read_stream(const vector *v, omt_buf *out) {
    char path[256];
    snprintf(path, sizeof(path), VECTOR_DIR "%s", v->stream);
    omt_err err;
    return omt_read_bounded(path, VMX_MAX_COMPRESSED_BYTES, out, &err) == OMT_READ_OK;
}

static vmx_decoder *make(const vector *v, size_t workers) {
    vmx_decoder *d = NULL;
    CHECK_INT(vmx_decoder_new(v->width, v->height,
                              vmx_color_space_resolve(v->color_space, v->height), workers, &d),
              VMX_OK);
    return d;
}

static void decodes_every_vector_bit_exactly(void) {
    vector vectors[32];
    size_t n = load_vectors(vectors, 32);
    CHECK(n > 0);
    for (size_t i = 0; i < n; i++) {
        omt_buf compressed;
        CHECK(read_stream(&vectors[i], &compressed));
        size_t workers_list[] = {1, 2, 3, 8};
        for (size_t w = 0; w < 4; w++) {
            vmx_decoder *d = make(&vectors[i], workers_list[w]);
            if (!d) continue;
            size_t uyvy_len = vectors[i].width * vectors[i].height * 2;
            uint8_t *uyvy = test_alloc(uyvy_len);
            CHECK_INT(vmx_decoder_load(d, compressed.data, compressed.len), VMX_OK);
            CHECK_INT(vmx_decode_uyvy(d, uyvy, uyvy_len, vectors[i].width * 2), VMX_OK);
            char hex[65];
            test_sha256_hex(uyvy, uyvy_len, hex);
            CHECK_MSG(strcmp(hex, vectors[i].uyvy) == 0, "%s UYVY with %zu workers",
                      vectors[i].label, workers_list[w]);
            free(uyvy);

            size_t bgrx_len = vectors[i].width * vectors[i].height * 4;
            uint8_t *bgrx = test_alloc(bgrx_len);
            CHECK_INT(vmx_decoder_load(d, compressed.data, compressed.len), VMX_OK);
            CHECK_INT(vmx_decode_bgrx(d, bgrx, bgrx_len, vectors[i].width * 4), VMX_OK);
            test_sha256_hex(bgrx, bgrx_len, hex);
            CHECK_MSG(strcmp(hex, vectors[i].bgrx) == 0, "%s BGRX with %zu workers",
                      vectors[i].label, workers_list[w]);
            free(bgrx);
            vmx_decoder_free(d);
        }
        omt_buf_free(&compressed);
    }
}

static void repeated_lifecycles_are_stable(void) {
    vector vectors[32];
    size_t n = load_vectors(vectors, 32);
    const vector *v = NULL;
    for (size_t i = 0; i < n && !v; i++)
        if (vectors[i].width == 1920) v = &vectors[i];
    CHECK(v != NULL);
    if (!v) return;
    omt_buf compressed;
    CHECK(read_stream(v, &compressed));
    vmx_decoder *d = make(v, 4);
    size_t len = v->width * v->height * 4;
    uint8_t *out = test_alloc(len);
    for (int round = 0; round < 8; round++) {
        CHECK_INT(vmx_decoder_load(d, compressed.data, compressed.len), VMX_OK);
        CHECK_INT(vmx_decode_bgrx(d, out, len, v->width * 4), VMX_OK);
        char hex[65];
        test_sha256_hex(out, len, hex);
        CHECK_STR(hex, v->bgrx);
    }
    free(out);
    vmx_decoder_free(d);
    omt_buf_free(&compressed);
}

static void rejects_malformed_and_truncated_streams(void) {
    vector vectors[32];
    CHECK(load_vectors(vectors, 32) > 0);
    const vector *v = &vectors[0];
    omt_buf compressed;
    CHECK(read_stream(v, &compressed));
    vmx_decoder *d = make(v, 1);
    CHECK_INT(vmx_decoder_load(d, NULL, 0), VMX_EMPTY);
    CHECK_INT(vmx_decoder_load(d, (const uint8_t *)"\1\2\3", 3), VMX_TRUNCATED);
    CHECK_INT(vmx_decoder_load(d, (const uint8_t *)"\x09\0\0\0\0", 5), VMX_INVALID_FORMAT);
    CHECK_INT(vmx_decoder_load(d, (const uint8_t *)"\x02\0\0\0\0", 5), VMX_UNSUPPORTED_FORMAT);

    size_t at = compressed.data[0] == 3 ? 4 : 2;
    uint8_t *wrong = test_alloc(compressed.len);
    memcpy(wrong, compressed.data, compressed.len);
    wrong[at] = (uint8_t)(wrong[at] + 1);
    CHECK_INT(vmx_decoder_load(d, wrong, compressed.len), VMX_SLICE_COUNT);

    size_t out_len = v->width * v->height * 4;
    uint8_t *out = test_alloc(out_len);
    for (size_t length = 1; length < compressed.len; length++)
        if (vmx_decoder_load(d, compressed.data, length) == VMX_OK)
            (void)vmx_decode_bgrx(d, out, out_len, v->width * 4);
    for (size_t pos = 5; pos < compressed.len; pos += 17) {
        memcpy(wrong, compressed.data, compressed.len);
        wrong[pos] ^= 0xA5;
        if (vmx_decoder_load(d, wrong, compressed.len) == VMX_OK)
            (void)vmx_decode_bgrx(d, out, out_len, v->width * 4);
    }
    free(out);
    free(wrong);
    vmx_decoder_free(d);
    omt_buf_free(&compressed);
}

static void rejects_undersized_destinations(void) {
    vector vectors[32];
    CHECK(load_vectors(vectors, 32) > 0);
    const vector *v = &vectors[0];
    omt_buf compressed;
    CHECK(read_stream(v, &compressed));
    vmx_decoder *d = make(v, 1);
    CHECK_INT(vmx_decoder_load(d, compressed.data, compressed.len), VMX_OK);
    size_t len = v->width * v->height * 4;
    uint8_t *out = test_alloc(len);
    CHECK_INT(vmx_decode_bgrx(d, out, len - 1, v->width * 4), VMX_OUTPUT_SIZE);
    CHECK_INT(vmx_decode_bgrx(d, out, len, v->width * 4 - 1), VMX_OUTPUT_SIZE);
    free(out);
    vmx_decoder_free(d);
    omt_buf_free(&compressed);
}

static void rejects_unsupported_geometry(void) {
    size_t cases[][3] = {{8, 32, 1},  {1922, 1080, 1}, {64, 8, 1}, {64, 1082, 1},
                         {65, 32, 1}, {64, 32, 0},     {64, 32, 9}};
    for (size_t i = 0; i < OMT_ARRAY_LEN(cases); i++) {
        vmx_decoder *d = NULL;
        CHECK_MSG(vmx_decoder_new(cases[i][0], cases[i][1], VMX_BT709, cases[i][2], &d) != VMX_OK,
                  "case %zu", i);
        CHECK(d == NULL);
    }
}

static vmx_bits reader(const uint8_t *bytes, size_t len) {
    vmx_bits r;
    CHECK(vmx_bits_init(&r, 64));
    CHECK(vmx_bits_load(&r, bytes, len));
    return r;
}

static void bit_reader_matches_the_reference(void) {
    uint8_t a[] = {0xB2, 0, 0, 0};
    vmx_bits r = reader(a, 4);
    CHECK_INT(vmx_bits_bit(&r), 1);
    CHECK_INT(vmx_bits_bit(&r), 0);
    CHECK_INT(vmx_bits_bit(&r), 1);
    CHECK_INT(vmx_bits_bit(&r), 1);
    CHECK_INT(vmx_bits_bits(&r, 4), 2);
    CHECK(!r.corrupt);
    vmx_bits_free(&r);

    uint8_t b[] = {0x04};
    r = reader(b, 1);
    CHECK_INT(vmx_bits_zeros(&r), 5);
    CHECK_INT(vmx_bits_bit(&r), 1);
    CHECK(!r.corrupt);
    vmx_bits_free(&r);

    uint8_t z[] = {0};
    r = reader(z, 1);
    for (int i = 0; i < 8; i++) {
        (void)vmx_bits_bits(&r, 8);
        vmx_bits_reload(&r);
    }
    CHECK(!r.corrupt);
    vmx_bits_free(&r);

    r = reader(z, 1);
    for (int i = 0; i < 64; i++) {
        (void)vmx_bits_bits(&r, 64);
        vmx_bits_reload(&r);
    }
    CHECK(r.corrupt);
    vmx_bits_free(&r);

    vmx_bits big;
    CHECK(vmx_bits_init(&big, 1000000));
    CHECK(big.allocated <= VMX_PADDING);
    uint8_t four[] = {0x80, 0, 0, 0};
    CHECK(vmx_bits_load(&big, four, 4));
    CHECK_INT(big.length, 4 + VMX_PADDING);
    vmx_bits_free(&big);

    vmx_bits small;
    CHECK(vmx_bits_init(&small, 1024));
    uint8_t lng[200];
    memset(lng, 0xFF, sizeof(lng));
    lng[0] = 0x80;
    CHECK(vmx_bits_load(&small, lng, sizeof(lng)));
    uint8_t one[] = {0x80};
    CHECK(vmx_bits_load(&small, one, 1));
    CHECK_INT(small.length, 1 + VMX_PADDING);
    for (int i = 0; i < 64; i++) {
        (void)vmx_bits_bits(&small, 64);
        vmx_bits_reload(&small);
    }
    CHECK(small.corrupt);
    vmx_bits_free(&small);
}

static void plane_helpers_match_the_reference(void) {
    CHECK_INT(vmx_mag_sign(1), 0);
    CHECK_INT(vmx_mag_sign(2), -1);
    CHECK_INT(vmx_mag_sign(3), 1);
    CHECK_INT(vmx_mag_sign(4), -2);
    CHECK_INT(vmx_mag_sign(5), 2);
    CHECK_INT(vmx_shift_signed(-3, 0), -3);
    CHECK_INT(vmx_shift_signed(-3, 16), 0);
    CHECK_INT(vmx_shift_signed(1, 3), 8);
    CHECK_INT(vmx_shift_signed(-1, 1), -2);
}

static uint64_t xorshift(uint64_t *s) {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    return *s;
}

/* On AArch64 this pins the NEON kernels to the portable ones; elsewhere the
 * dispatch resolves to the portable kernel and the check is trivially true,
 * which is why the suite also runs under emulated AArch64. */
static void idct_kernels_agree_bit_for_bit(void) {
    uint64_t state = 0x2545F4914F6CDD1Dull;
    for (int round = 0; round < 2000; round++) {
        int16_t block[64];
        for (int i = 0; i < 64; i++) {
            uint64_t raw = xorshift(&state);
            switch (round % 4) {
            case 0: block[i] = (int16_t)((int)(raw % 512) - 256); break;
            case 1: block[i] = i == 0 ? (int16_t)((int)(raw % 4096) - 2048) : 0; break;
            case 2: block[i] = (int16_t)(uint16_t)(raw & 0xFFFF); break;
            default: block[i] = raw % 8 == 0 ? INT16_MAX : raw % 8 == 1 ? INT16_MIN : 0;
            }
        }
        uint16_t matrix[64];
        vmx_decode_matrix((size_t)(round % 25), matrix);
        int16_t biases[] = {0, 128};
        for (int b = 0; b < 2; b++) {
            uint8_t expected[64], actual[64];
            vmx_idct_scalar(block, matrix, expected, 8, biases[b]);
            vmx_idct(block, matrix, actual, 8, biases[b]);
            CHECK_MSG(memcmp(expected, actual, 64) == 0, "round %d bias %d", round, biases[b]);
        }
    }
}

static void convert_kernels_agree_bit_for_bit(void) {
    uint64_t state = 0x123456789ABCDEF1ull;
    size_t widths[] = {2, 4, 6, 8, 14, 16, 18, 30, 32, 34, 62, 64, 96, 130};
    for (size_t w = 0; w < OMT_ARRAY_LEN(widths); w++) {
        size_t width = widths[w], pairs = width / 2;
        uint8_t luma[256] = {0}, blue[128] = {0}, red[128] = {0}, expected[1024], actual[1024];
        for (size_t i = 0; i < width; i++) luma[i] = (uint8_t)(xorshift(&state) >> 24);
        for (size_t i = 0; i < pairs; i++) blue[i] = (uint8_t)(xorshift(&state) >> 24);
        for (size_t i = 0; i < pairs; i++) red[i] = (uint8_t)(xorshift(&state) >> 24);
        const int16_t *coefficients[] = {vmx_yuv_rgb_709, vmx_yuv_rgb_601};
        for (int c = 0; c < 2; c++) {
            vmx_bgra_row_scalar(luma, blue, red, width, expected, coefficients[c]);
            vmx_bgra_row(luma, blue, red, width, actual, coefficients[c]);
            CHECK_MSG(memcmp(expected, actual, width * 4) == 0, "width %zu", width);
        }
    }
    uint8_t lv[] = {0, 16, 128, 235, 255}, cv[] = {0, 1, 128, 254, 255};
    for (int a = 0; a < 5; a++)
        for (int b = 0; b < 5; b++) {
            uint8_t luma[64], blue[32], red[32], expected[256], actual[256];
            memset(luma, lv[a], 64);
            memset(blue, cv[b], 32);
            memset(red, 255 - cv[b], 32);
            vmx_bgra_row_scalar(luma, blue, red, 64, expected, vmx_yuv_rgb_709);
            vmx_bgra_row(luma, blue, red, 64, actual, vmx_yuv_rgb_709);
            CHECK(memcmp(expected, actual, 256) == 0);
        }
}

int main(void) {
    RUN(decodes_every_vector_bit_exactly);
    RUN(repeated_lifecycles_are_stable);
    RUN(rejects_malformed_and_truncated_streams);
    RUN(rejects_undersized_destinations);
    RUN(rejects_unsupported_geometry);
    RUN(bit_reader_matches_the_reference);
    RUN(plane_helpers_match_the_reference);
    RUN(idct_kernels_agree_bit_for_bit);
    RUN(convert_kernels_agree_bit_for_bit);
    return TEST_EXIT();
}
