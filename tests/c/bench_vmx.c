/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Decode throughput. A measurement, not a gate, so it is not one of the
 * suites; it reports per worker count so a regression in single-core cost
 * cannot hide behind the pool.
 *
 *   make -f mk/c.mk BUILD=release bench
 *
 * Run this on each supported board. The per-board ceilings in
 * deploy/lib/board-profile.sh are targets derived from core count and clock,
 * and this is what confirms or refutes them. The receiver gives the decoder
 * three of the four cores every supported board has, so the 3-worker row
 * decides a tier: if it cannot sustain the frame interval its ceiling
 * promises, lower that board's profile.
 *
 * Only 1080p and 480p geometries are committed as vectors, so a 720p tier is
 * bracketed rather than measured directly. VMX_VECTOR_DIR overrides where the
 * vectors are read from, for a run on a board without the checkout.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "vmx/vmx.h"

#define FRAMES 60

static const struct {
    const char *label;
    size_t width, height;
    vmx_color_space color_space;
} CASES[] = {
    {"gradient-1920x1080-709", 1920, 1080, VMX_BT709},
    {"flat-1920x1080-709", 1920, 1080, VMX_BT709},
    {"gradient-720x480-601", 720, 480, VMX_BT601},
};

static double now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec * 1000.0 + (double)ts.tv_nsec / 1e6;
}

static uint8_t *read_vector(const char *label, size_t *len) {
    const char *dir = getenv("VMX_VECTOR_DIR");
    char path[1024];
    snprintf(path, sizeof(path), "%s/%s.vmx", dir ? dir : "tests/vectors/vmx", label);
    FILE *f = fopen(path, "rb");
    if (!f) {
        fprintf(stderr, "cannot open %s\n", path);
        exit(1);
    }
    uint8_t *data = malloc(VMX_MAX_COMPRESSED_BYTES);
    size_t n = data ? fread(data, 1, VMX_MAX_COMPRESSED_BYTES, f) : 0;
    fclose(f);
    if (!data || n == 0) {
        fprintf(stderr, "cannot read %s\n", path);
        exit(1);
    }
    *len = n;
    return data;
}

int main(void) {
    printf("\nBGRX decode, %d frames per measurement\n", FRAMES);
    printf("The receiver uses 3 workers; that row decides a board's tier.\n\n");
    static const size_t WORKERS[] = {1, 3, 4};
    for (size_t c = 0; c < sizeof(CASES) / sizeof(CASES[0]); c++) {
        size_t width = CASES[c].width, height = CASES[c].height;
        printf("%s (%zux%zu)\n", CASES[c].label, width, height);
        size_t len;
        uint8_t *compressed = read_vector(CASES[c].label, &len);
        uint8_t *output = malloc(width * height * 4);
        if (!output) return 1;
        for (size_t w = 0; w < 3; w++) {
            vmx_decoder *decoder;
            if (vmx_decoder_new(width, height, CASES[c].color_space, WORKERS[w], &decoder) !=
                VMX_OK) {
                fprintf(stderr, "cannot create a decoder\n");
                return 1;
            }
            double start = 0;
            for (int frame = -5; frame < FRAMES; frame++) {
                if (frame == 0) start = now_ms();
                vmx_status st = vmx_decoder_load(decoder, compressed, len);
                if (st == VMX_OK)
                    st = vmx_decode_bgrx(decoder, output, width * height * 4, width * 4);
                if (st != VMX_OK) {
                    fprintf(stderr, "decode failed: %s\n", vmx_status_name(st));
                    return 1;
                }
            }
            double per_frame = (now_ms() - start) / FRAMES;
            printf("  %zu worker(s): %6.2f ms/frame  %6.1f fps   60 fps: %s  30 fps: %s\n",
                   WORKERS[w], per_frame, 1000.0 / per_frame,
                   per_frame <= 1000.0 / 60.0 ? "ok" : "OVER",
                   per_frame <= 1000.0 / 30.0 ? "ok" : "OVER");
            vmx_decoder_free(decoder);
        }
        free(output);
        free(compressed);
        printf("\n");
    }
    return 0;
}
