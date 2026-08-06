/* Copyright (c) 2026 Matthew David Miller; SPDX-License-Identifier: MIT */
#include "vmxcodec.h"

#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define TEST_WIDTH 320
#define TEST_HEIGHT 180
#define TEST_STRIDE (TEST_WIDTH * 2)
#define TEST_THREADS 4
#define TEST_CYCLES 8
#define ENCODED_CAPACITY ((size_t)TEST_WIDTH * (size_t)TEST_HEIGHT * 4u)

typedef struct {
    BYTE *encoded;
    BYTE *decoded;
    int encoded_length;
    uint64_t encoded_checksum;
    uint64_t decoded_checksum;
} vmx_reference;

static void fail(const char *message)
{
    (void)fprintf(stderr, "FAIL: %s\n", message);
    exit(EXIT_FAILURE);
}

static void require_true(int condition, const char *message)
{
    if (!condition) {
        fail(message);
    }
}

static uint64_t fnv1a64(const BYTE *data, size_t length)
{
    uint64_t hash = UINT64_C(14695981039346656037);
    for (size_t index = 0u; index < length; ++index) {
        hash ^= data[index];
        hash *= UINT64_C(1099511628211);
    }
    return hash;
}

static void fill_representative_uyvy(BYTE *pixels)
{
    for (int y = 0; y < TEST_HEIGHT; ++y) {
        BYTE *row = pixels + ((size_t)y * TEST_STRIDE);
        for (int x = 0; x < TEST_WIDTH; x += 2) {
            const unsigned pair = (unsigned)x / 2u;
            row[x * 2] = (BYTE)(32u + ((pair * 11u + (unsigned)y * 3u) % 192u));
            row[x * 2 + 1] = (BYTE)(16u + (((unsigned)x * 5u + (unsigned)y * 7u) % 220u));
            row[x * 2 + 2] = (BYTE)(24u + ((pair * 13u + (unsigned)y * 5u) % 208u));
            row[x * 2 + 3] =
                (BYTE)(16u + (((unsigned)x * 9u + (unsigned)y * 11u + 37u) % 220u));
        }
    }
}

static void run_codec_cycle(const BYTE *source, vmx_reference *reference, int establish_reference)
{
    const VMX_SIZE dimensions = {TEST_WIDTH, TEST_HEIGHT};
    const size_t image_length = (size_t)TEST_STRIDE * TEST_HEIGHT;
    BYTE *encoded = (BYTE *)malloc(ENCODED_CAPACITY);
    BYTE *decoded = (BYTE *)malloc(image_length);
    VMX_INSTANCE *encoder = NULL;
    VMX_INSTANCE *decoder = NULL;

    require_true(encoded != NULL && decoded != NULL, "test buffer allocation succeeds");
    memset(decoded, 0, image_length);

    encoder = VMX_Create(dimensions, VMX_PROFILE_DEFAULT, VMX_COLORSPACE_UNDEFINED);
    decoder = VMX_Create(dimensions, VMX_PROFILE_DEFAULT, VMX_COLORSPACE_UNDEFINED);
    require_true(encoder != NULL && decoder != NULL, "codec instances are created");

    VMX_SetThreads(encoder, TEST_THREADS);
    VMX_SetThreads(decoder, TEST_THREADS);
    require_true(VMX_GetThreads(encoder) == TEST_THREADS, "encoder uses requested thread count");
    require_true(VMX_GetThreads(decoder) == TEST_THREADS, "decoder uses requested thread count");
    require_true(
        VMX_EncodeUYVY(encoder, (BYTE *)source, TEST_STRIDE, 0) == VMX_ERR_OK,
        "packed UYVY frame encodes");

    const int encoded_length = VMX_SaveTo(encoder, encoded, (int)ENCODED_CAPACITY);
    require_true(encoded_length > 16, "encoded frame has a bounded nonempty payload");
    require_true(
        VMX_LoadFrom(decoder, encoded, encoded_length) == VMX_ERR_OK,
        "encoded frame loads");
    require_true(
        VMX_DecodeUYVY(decoder, decoded, TEST_STRIDE) == VMX_ERR_OK,
        "loaded frame decodes to packed UYVY");

    const uint64_t encoded_checksum = fnv1a64(encoded, (size_t)encoded_length);
    const uint64_t decoded_checksum = fnv1a64(decoded, image_length);
    require_true(encoded_checksum != 0u && decoded_checksum != 0u, "codec checksums are nonzero");

    if (establish_reference) {
        reference->encoded = encoded;
        reference->decoded = decoded;
        reference->encoded_length = encoded_length;
        reference->encoded_checksum = encoded_checksum;
        reference->decoded_checksum = decoded_checksum;
        encoded = NULL;
        decoded = NULL;
    } else {
        require_true(
            encoded_length == reference->encoded_length,
            "repeated encoding has stable payload length");
        require_true(
            encoded_checksum == reference->encoded_checksum &&
                memcmp(encoded, reference->encoded, (size_t)encoded_length) == 0,
            "repeated multithreaded encoding is byte-equivalent");
        require_true(
            decoded_checksum == reference->decoded_checksum &&
                memcmp(decoded, reference->decoded, image_length) == 0,
            "repeated multithreaded decoding is byte-equivalent");
    }

    VMX_Destroy(decoder);
    VMX_Destroy(encoder);
    free(decoded);
    free(encoded);
}

static void malformed_stream_contract(const BYTE *encoded, int encoded_length)
{
    const VMX_SIZE dimensions = {TEST_WIDTH, TEST_HEIGHT};
    BYTE *mutated = (BYTE *)malloc((size_t)encoded_length);
    VMX_INSTANCE *decoder =
        VMX_Create(dimensions, VMX_PROFILE_DEFAULT, VMX_COLORSPACE_UNDEFINED);

    require_true(mutated != NULL && decoder != NULL, "malformed-stream fixtures are created");
    require_true(
        VMX_LoadFrom(decoder, NULL, encoded_length) == VMX_ERR_INVALID_PARAMETERS,
        "null compressed input is rejected");
    require_true(
        VMX_LoadFrom(decoder, (BYTE *)encoded, 0) == VMX_ERR_INVALID_PARAMETERS,
        "empty compressed input is rejected");
    require_true(
        VMX_LoadFrom(decoder, (BYTE *)encoded, 1) == VMX_ERR_BUFFER_OVERFLOW,
        "truncated codec header is rejected");
    require_true(
        VMX_LoadFrom(decoder, (BYTE *)encoded, encoded_length - 1) == VMX_ERR_BUFFER_OVERFLOW,
        "truncated final payload is rejected");

    memcpy(mutated, encoded, (size_t)encoded_length);
    mutated[0] = 0xffu;
    require_true(
        VMX_LoadFrom(decoder, mutated, encoded_length) == VMX_ERR_INVALID_CODEC_FORMAT,
        "unknown codec format is rejected");

    memcpy(mutated, encoded, (size_t)encoded_length);
    mutated[2] ^= 1u;
    require_true(
        VMX_LoadFrom(decoder, mutated, encoded_length) == VMX_ERR_INVALID_SLICE_COUNT,
        "mismatched slice count is rejected");

    VMX_Destroy(decoder);
    free(mutated);
}

int main(void)
{
    const size_t image_length = (size_t)TEST_STRIDE * TEST_HEIGHT;
    BYTE *source = (BYTE *)malloc(image_length);
    vmx_reference reference = {0};

    require_true(source != NULL, "source allocation succeeds");
    fill_representative_uyvy(source);
    run_codec_cycle(source, &reference, 1);
    malformed_stream_contract(reference.encoded, reference.encoded_length);

    for (int cycle = 1; cycle < TEST_CYCLES; ++cycle) {
        run_codec_cycle(source, &reference, 0);
    }

    (void)printf(
        "VMX C17 contract passed: %d cycles, %d-byte stream, encoded=%016" PRIx64
        ", decoded=%016" PRIx64 "\n",
        TEST_CYCLES,
        reference.encoded_length,
        reference.encoded_checksum,
        reference.decoded_checksum);

    free(reference.decoded);
    free(reference.encoded);
    free(source);
    return EXIT_SUCCESS;
}
