/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Decode-only VMX1 for the appliance. The encoder, the preview resolutions,
 * and the unused colour-conversion outputs of the reference codec are
 * intentionally absent; see third_party/omt/PROVENANCE.md.
 *
 * A decoder is built for one fixed frame geometry. Frames are loaded (the
 * envelope is validated and split into per-slice bitstreams) and then decoded
 * into packed UYVY or BGRX. Slices are decoded in parallel by a bounded pool;
 * the calling thread is one of the workers, so a pool of N decodes on N
 * threads with N-1 spawned.
 */
#ifndef OMT_VMX_H
#define OMT_VMX_H

#include "common/base.h"

#define VMX_WORKER_STACK_SIZE (128u * 1024u)
#define VMX_MAX_WIDTH 1920u
#define VMX_MAX_HEIGHT 1080u
#define VMX_MAX_COMPRESSED_BYTES (10u * 1024u * 1024u)
#define VMX_MAX_WORKERS 8u

typedef enum {
    VMX_OK = 0,
    VMX_INVALID_DIMENSIONS,
    VMX_EMPTY,
    VMX_OVERSIZED,
    VMX_TRUNCATED,
    VMX_INVALID_FORMAT,
    VMX_UNSUPPORTED_FORMAT,
    VMX_SLICE_COUNT,
    VMX_OUTPUT_SIZE,
    VMX_WORKER_FAILURE,
    VMX_CORRUPT_STREAM,
} vmx_status;

/* The Rust decoder's error names, which status files and logs carry. */
const char *vmx_status_name(vmx_status status);

typedef enum { VMX_BT601, VMX_BT709 } vmx_color_space;

/* Mirrors the reference codec's undefined-colour-space default. */
vmx_color_space vmx_color_space_resolve(int32_t value, size_t height);

typedef struct vmx_decoder vmx_decoder;

vmx_status vmx_decoder_new(size_t width, size_t height, vmx_color_space color_space, size_t workers,
                           vmx_decoder **out);
void vmx_decoder_free(vmx_decoder *decoder);
/* VMX_LoadFrom: validates the envelope and splits it into slice streams. */
vmx_status vmx_decoder_load(vmx_decoder *decoder, const uint8_t *input, size_t len);
vmx_status vmx_decode_uyvy(vmx_decoder *decoder, uint8_t *output, size_t output_len, size_t stride);
/* BGRX is the byte order the DRM scanout reads as XRGB8888. */
vmx_status vmx_decode_bgrx(vmx_decoder *decoder, uint8_t *output, size_t output_len, size_t stride);

/* One plane of a planar output: `len` bytes at `data`, rows `stride` apart. */
typedef struct {
    uint8_t *data;
    size_t len;
    size_t stride;
} vmx_plane;

/* Planar YUV 4:2:2 as the decoder holds it: luma, then Cb, then Cr at half
 * width, each row copied once with no conversion. This is DRM_FORMAT_YUV422,
 * which display hardware that converts YCbCr itself scans out directly, at
 * half the bytes of BGRX. */
vmx_status vmx_decode_yuv422p(vmx_decoder *decoder, const vmx_plane planes[3]);

/* The largest placed area a decode accepts, which bounds each worker's row
 * buffer. Wider than any mode a supported board drives. */
#define VMX_MAX_PLACED_WIDTH 8192u
#define VMX_MAX_PLACED_HEIGHT 8192u

/* Nearest-neighbour placement of the picture into a destination of another
 * size: destination pixel (x + i, y + j) takes source pixel (columns[i],
 * rows[j]). `rows` must be non-decreasing; every entry must lie inside the
 * decoder's geometry. */
typedef struct {
    size_t x, y, width, height;
    const uint32_t *columns; /* `width` entries */
    const uint32_t *rows;    /* `height` entries */
} vmx_placement;

/* BGRX, resampled into `placement` within the output. Each worker converts
 * only the source rows its slices contribute and stores each placed row once,
 * so the resample runs on the whole pool and no full-size intermediate frame
 * is written. Pixels outside the placed area are left untouched. */
vmx_status vmx_decode_bgrx_placed(vmx_decoder *decoder, uint8_t *output, size_t output_len,
                                  size_t stride, const vmx_placement *placement);

#endif
