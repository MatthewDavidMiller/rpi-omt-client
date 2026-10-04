/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The decoder envelope and its worker pool.
 *
 * Slices are claimed one at a time from a shared counter rather than handed
 * out as fixed bands: slice cost follows picture detail, so fixed bands made
 * every frame wait for whichever worker drew the busiest part of the picture.
 * The calling thread claims slices too, so a decode never waits on a thread
 * hand-off it could have done itself.
 *
 * Each slice index is claimed exactly once, so every thread writes disjoint
 * slice state and a disjoint output band. The caller does not return -- and so
 * cannot reuse or free the output or the slices -- until every worker has
 * reported the frame finished, and teardown joins the workers before the
 * slices are freed.
 */
#include "vmx/vmx.h"

#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>

#include "vmx/internal.h"
#include "vmx/plane.h"

#define CODEC_FORMAT_PROGRESSIVE 1
#define CODEC_FORMAT_INTERLACED 2
#define CODEC_FORMAT_EXTENDED 3

typedef struct {
    vmx_slice_streams streams;
    size_t rows; /* rows of this slice inside the visible image */
} vmx_slice;

/* Reused YUV planes for one slice at a time. */
typedef struct {
    uint8_t *luma;
    uint8_t *blue;
    uint8_t *red;
} plane_scratch;

typedef enum { PIXELS_UYVY, PIXELS_BGRX } pixel_format;

typedef struct {
    uint8_t *output;
    size_t output_len;
    size_t stride;
    pixel_format pixels;
    const int16_t *coefficients;
} frame_job;

struct vmx_decoder;

typedef struct {
    struct vmx_decoder *decoder;
    size_t index;
} worker_arg;

struct vmx_decoder {
    size_t width;
    size_t height;
    vmx_color_space color_space;
    size_t luma_stride;
    size_t chroma_stride;
    vmx_slice *slices;
    size_t slice_count;
    size_t workers;
    uint16_t matrix[64];
    int32_t dc_shift;
    bool loaded;
    plane_scratch scratch[VMX_MAX_WORKERS];

    /* The pool: workers - 1 threads, scratch[i + 1] belonging to thread i. */
    pthread_t threads[VMX_MAX_WORKERS];
    worker_arg args[VMX_MAX_WORKERS];
    size_t thread_count;
    bool sync_ready;
    pthread_mutex_t lock;
    pthread_cond_t start;
    pthread_cond_t finished;
    uint64_t generation;
    size_t running;
    bool shutdown;
    frame_job job;
    atomic_size_t next;
    atomic_bool failed;
};

const char *vmx_status_name(vmx_status status) {
    switch (status) {
    case VMX_OK: return "Ok";
    case VMX_INVALID_DIMENSIONS: return "InvalidDimensions";
    case VMX_EMPTY: return "Empty";
    case VMX_OVERSIZED: return "Oversized";
    case VMX_TRUNCATED: return "Truncated";
    case VMX_INVALID_FORMAT: return "InvalidFormat";
    case VMX_UNSUPPORTED_FORMAT: return "UnsupportedFormat";
    case VMX_SLICE_COUNT: return "SliceCount";
    case VMX_OUTPUT_SIZE: return "OutputSize";
    case VMX_WORKER_FAILURE: return "WorkerFailure";
    case VMX_CORRUPT_STREAM: return "CorruptStream";
    }
    return "Unknown";
}

vmx_color_space vmx_color_space_resolve(int32_t value, size_t height) {
    if (value == 601) return VMX_BT601;
    if (value == 709) return VMX_BT709;
    return height >= 720 ? VMX_BT709 : VMX_BT601;
}

static size_t align8(size_t v) { return (v + 7u) & ~(size_t)7u; }

/* Decodes one slice into its band of the output. */
static bool decode_slice(vmx_decoder *d, size_t index, plane_scratch *scratch) {
    const frame_job *job = &d->job;
    vmx_slice *slice = &d->slices[index];
    size_t luma_len = d->luma_stride * VMX_SLICE_HEIGHT;
    size_t chroma_len = d->chroma_stride * VMX_SLICE_HEIGHT;
    if (!vmx_decode_plane(&slice->streams, d->luma_stride, 128, d->matrix, d->dc_shift,
                          scratch->luma, luma_len) ||
        !vmx_decode_plane(&slice->streams, d->chroma_stride, 0, d->matrix, d->dc_shift,
                          scratch->blue, chroma_len) ||
        !vmx_decode_plane(&slice->streams, d->chroma_stride, 0, d->matrix, d->dc_shift,
                          scratch->red, chroma_len))
        return false;

    size_t bytes_per_pixel = job->pixels == PIXELS_UYVY ? 2 : 4;
    size_t row_bytes = d->width * bytes_per_pixel;
    for (size_t row = 0; row < slice->rows; row++) {
        /* Every row is bounds-checked before the kernel runs, so no kernel can
         * be handed a rectangle it would run past. */
        size_t offset = (index * VMX_SLICE_HEIGHT + row) * job->stride;
        if (offset > job->output_len || job->output_len - offset < row_bytes) return false;
        const uint8_t *luma = scratch->luma + row * d->luma_stride;
        const uint8_t *blue = scratch->blue + row * d->chroma_stride;
        const uint8_t *red = scratch->red + row * d->chroma_stride;
        uint8_t *out = job->output + offset;
        if (job->pixels == PIXELS_UYVY)
            vmx_uyvy_row(luma, blue, red, d->width, out);
        else
            vmx_bgra_row(luma, blue, red, d->width, out, job->coefficients);
    }
    return true;
}

/* Decodes slices claimed from the shared counter until none are left. A
 * failed slice fails the frame and stops the claiming for every thread. */
static void decode_claimed(vmx_decoder *d, plane_scratch *scratch) {
    for (;;) {
        size_t index = atomic_fetch_add_explicit(&d->next, 1, memory_order_relaxed);
        if (index >= d->slice_count) return;
        if (!decode_slice(d, index, scratch)) {
            atomic_store_explicit(&d->failed, true, memory_order_relaxed);
            atomic_store_explicit(&d->next, d->slice_count, memory_order_relaxed);
            return;
        }
    }
}

static void *worker_main(void *raw) {
    worker_arg *arg = raw;
    vmx_decoder *d = arg->decoder;
    uint64_t seen = 0;
    pthread_mutex_lock(&d->lock);
    for (;;) {
        while (!d->shutdown && d->generation == seen) pthread_cond_wait(&d->start, &d->lock);
        if (d->shutdown) break;
        seen = d->generation;
        pthread_mutex_unlock(&d->lock);
        decode_claimed(d, &d->scratch[arg->index]);
        pthread_mutex_lock(&d->lock);
        if (--d->running == 0) pthread_cond_signal(&d->finished);
    }
    pthread_mutex_unlock(&d->lock);
    return NULL;
}

static bool scratch_init(plane_scratch *s, size_t luma_stride, size_t chroma_stride) {
    s->luma = calloc(luma_stride, VMX_SLICE_HEIGHT);
    s->blue = calloc(chroma_stride, VMX_SLICE_HEIGHT);
    s->red = calloc(chroma_stride, VMX_SLICE_HEIGHT);
    return s->luma && s->blue && s->red;
}

static void scratch_free(plane_scratch *s) {
    free(s->luma);
    free(s->blue);
    free(s->red);
}

void vmx_decoder_free(vmx_decoder *d) {
    if (!d) return;
    if (d->sync_ready) {
        pthread_mutex_lock(&d->lock);
        d->shutdown = true;
        pthread_cond_broadcast(&d->start);
        pthread_mutex_unlock(&d->lock);
        for (size_t i = 0; i < d->thread_count; i++) pthread_join(d->threads[i], NULL);
        pthread_cond_destroy(&d->start);
        pthread_cond_destroy(&d->finished);
        pthread_mutex_destroy(&d->lock);
    }
    for (size_t i = 0; i < d->slice_count; i++) {
        vmx_bits_free(&d->slices[i].streams.dc);
        vmx_bits_free(&d->slices[i].streams.ac);
    }
    free(d->slices);
    for (size_t i = 0; i < VMX_MAX_WORKERS; i++) scratch_free(&d->scratch[i]);
    free(d);
}

vmx_status vmx_decoder_new(size_t width, size_t height, vmx_color_space color_space, size_t workers,
                           vmx_decoder **out) {
    *out = NULL;
    if (width < 16 || width > VMX_MAX_WIDTH || width % 2 != 0 || height < 16 ||
        height > VMX_MAX_HEIGHT || workers == 0 || workers > VMX_MAX_WORKERS)
        return VMX_INVALID_DIMENSIONS;
    vmx_decoder *d = calloc(1, sizeof(*d));
    if (!d) return VMX_WORKER_FAILURE;
    d->width = width;
    d->height = height;
    d->color_space = color_space;
    d->luma_stride = align8(width);
    d->chroma_stride = align8(width / 2);
    size_t slice_count = (height + VMX_SLICE_HEIGHT - 1) / VMX_SLICE_HEIGHT;
    /* The reference codec sizes the per-slice streams from the luma stride. */
    size_t dc_capacity = d->luma_stride * VMX_SLICE_HEIGHT * 2;
    size_t ac_capacity = d->luma_stride * VMX_SLICE_HEIGHT * 4;
    d->slices = calloc(slice_count, sizeof(vmx_slice));
    if (!d->slices) goto fail;
    for (size_t i = 0; i < slice_count; i++) {
        size_t visible = height > i * VMX_SLICE_HEIGHT ? height - i * VMX_SLICE_HEIGHT : 0;
        d->slices[i].rows = visible < VMX_SLICE_HEIGHT ? visible : VMX_SLICE_HEIGHT;
        d->slice_count = i + 1;
        if (!vmx_bits_init(&d->slices[i].streams.dc, dc_capacity) ||
            !vmx_bits_init(&d->slices[i].streams.ac, ac_capacity))
            goto fail;
    }
    d->workers = workers < slice_count ? workers : slice_count;
    for (size_t i = 0; i < d->workers; i++)
        if (!scratch_init(&d->scratch[i], d->luma_stride, d->chroma_stride)) goto fail;
    vmx_decode_matrix(0, d->matrix);

    if (pthread_mutex_init(&d->lock, NULL) != 0) goto fail;
    if (pthread_cond_init(&d->start, NULL) != 0) {
        pthread_mutex_destroy(&d->lock);
        goto fail;
    }
    if (pthread_cond_init(&d->finished, NULL) != 0) {
        pthread_cond_destroy(&d->start);
        pthread_mutex_destroy(&d->lock);
        goto fail;
    }
    d->sync_ready = true;

    pthread_attr_t attr;
    if (pthread_attr_init(&attr) != 0) goto fail;
    pthread_attr_setstacksize(&attr, VMX_WORKER_STACK_SIZE);
    /* Workers never take a process signal; shutdown is the main thread's. */
    sigset_t all, previous;
    sigfillset(&all);
    pthread_sigmask(SIG_SETMASK, &all, &previous);
    bool spawned = true;
    for (size_t i = 0; i + 1 < d->workers; i++) {
        d->args[i].decoder = d;
        d->args[i].index = i + 1;
        if (pthread_create(&d->threads[i], &attr, worker_main, &d->args[i]) != 0) {
            spawned = false;
            break;
        }
        d->thread_count++;
#if defined(__GLIBC__) || defined(__linux__)
        char name[16];
        snprintf(name, sizeof(name), "vmx-decode-%u", (unsigned)(i % VMX_MAX_WORKERS));
        pthread_setname_np(d->threads[i], name);
#endif
    }
    pthread_sigmask(SIG_SETMASK, &previous, NULL);
    pthread_attr_destroy(&attr);
    if (!spawned) goto fail;
    *out = d;
    return VMX_OK;
fail:
    vmx_decoder_free(d);
    return VMX_WORKER_FAILURE;
}

/* Reads one length-prefixed slice stream at *cursor. */
static vmx_status take_stream(const uint8_t *input, size_t len, size_t *cursor,
                              const uint8_t **data, size_t *data_len) {
    if (*cursor > len || len - *cursor < 4) return VMX_TRUNCATED;
    size_t length = omt_le32(input + *cursor);
    size_t start = *cursor + 4;
    if (len - start < length) return VMX_TRUNCATED;
    *data = input + start;
    *data_len = length;
    *cursor = start + length;
    return VMX_OK;
}

vmx_status vmx_decoder_load(vmx_decoder *d, const uint8_t *input, size_t len) {
    d->loaded = false;
    if (len == 0) return VMX_EMPTY;
    if (len > VMX_MAX_COMPRESSED_BYTES) return VMX_OVERSIZED;
    if (len < 5) return VMX_TRUNCATED;
    if (input[0] == CODEC_FORMAT_INTERLACED) return VMX_UNSUPPORTED_FORMAT;
    if (input[0] != CODEC_FORMAT_PROGRESSIVE && input[0] != CODEC_FORMAT_EXTENDED)
        return VMX_INVALID_FORMAT;
    size_t offset = 0;
    int32_t dc_shift = 0;
    if (input[0] == CODEC_FORMAT_EXTENDED) {
        offset = 2;
        dc_shift = input[1];
    }
    if (offset + 2 >= len) return VMX_TRUNCATED;
    /* Interlaced frames use the reference's field-paired slice layout, which
     * the appliance's progressive pipeline never emits. */
    if (input[offset] != CODEC_FORMAT_PROGRESSIVE) return VMX_UNSUPPORTED_FORMAT;
    int32_t quality = input[offset + 1];
    size_t slices = input[offset + 2];
    if (slices != d->slice_count) return VMX_SLICE_COUNT;

    size_t cursor = 3 + offset;
    for (size_t i = 0; i < d->slice_count; i++) {
        const uint8_t *data;
        size_t data_len;
        vmx_status st = take_stream(input, len, &cursor, &data, &data_len);
        if (st != VMX_OK) return st;
        if (!vmx_bits_load(&d->slices[i].streams.dc, data, data_len)) return VMX_OVERSIZED;
    }
    /* A preview-only frame carries the DC streams alone. */
    bool has_ac = cursor < len;
    for (size_t i = 0; i < d->slice_count; i++) {
        const uint8_t *data = NULL;
        size_t data_len = 0;
        if (has_ac) {
            vmx_status st = take_stream(input, len, &cursor, &data, &data_len);
            if (st != VMX_OK) return st;
        }
        if (!vmx_bits_load(&d->slices[i].streams.ac, data, data_len)) return VMX_OVERSIZED;
    }
    vmx_decode_matrix(vmx_quality_index(quality), d->matrix);
    d->dc_shift = dc_shift;
    d->loaded = true;
    return VMX_OK;
}

static vmx_status decode(vmx_decoder *d, uint8_t *output, size_t output_len, size_t stride,
                         size_t minimum_stride, pixel_format pixels) {
    if (!d->loaded) return VMX_EMPTY;
    size_t needed;
    if (stride < minimum_stride || !omt_mul(stride, d->height, &needed) || output_len < needed)
        return VMX_OUTPUT_SIZE;
    for (size_t i = 0; i < d->slice_count; i++) {
        vmx_bits_reset(&d->slices[i].streams.dc);
        vmx_bits_reset(&d->slices[i].streams.ac);
    }
    d->job.output = output;
    d->job.output_len = output_len;
    d->job.stride = stride;
    d->job.pixels = pixels;
    d->job.coefficients = d->color_space == VMX_BT601 ? vmx_yuv_rgb_601 : vmx_yuv_rgb_709;
    atomic_store_explicit(&d->next, 0, memory_order_relaxed);
    atomic_store_explicit(&d->failed, false, memory_order_relaxed);

    if (d->thread_count == 0) {
        decode_claimed(d, &d->scratch[0]);
    } else {
        /* The job is published under the lock the workers wake on, which
         * orders every write above before their first claim. */
        pthread_mutex_lock(&d->lock);
        d->running = d->thread_count;
        d->generation++;
        pthread_cond_broadcast(&d->start);
        pthread_mutex_unlock(&d->lock);
        decode_claimed(d, &d->scratch[0]);
        pthread_mutex_lock(&d->lock);
        while (d->running > 0) pthread_cond_wait(&d->finished, &d->lock);
        pthread_mutex_unlock(&d->lock);
    }
    return atomic_load_explicit(&d->failed, memory_order_relaxed) ? VMX_CORRUPT_STREAM : VMX_OK;
}

vmx_status vmx_decode_uyvy(vmx_decoder *d, uint8_t *output, size_t output_len, size_t stride) {
    return decode(d, output, output_len, stride, d->width * 2, PIXELS_UYVY);
}

vmx_status vmx_decode_bgrx(vmx_decoder *d, uint8_t *output, size_t output_len, size_t stride) {
    return decode(d, output, output_len, stride, d->width * 4, PIXELS_BGRX);
}
