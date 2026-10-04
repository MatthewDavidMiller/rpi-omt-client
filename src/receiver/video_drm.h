/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Direct KMS scanout through the legacy mode-setting ioctls. The receiver
 * owns the CRTC, decodes each VMX frame straight into a dumb buffer, and
 * page-flips. Mode selection re-runs whenever the incoming format changes, a
 * format the display cannot show is reported as unsupported rather than as a
 * failure, and when no mode carries the format the closest usable one is
 * taken and each frame is resampled into it.
 */
#ifndef OMT_RECEIVER_VIDEO_DRM_H
#define OMT_RECEIVER_VIDEO_DRM_H

#include "receiver/channel.h"
#include "receiver/scale.h"
#include "receiver_core/core.h"
#include "vmx/vmx.h"

/* Buffers in the flip chain: one scanning out, one queued, one decoding. */
#define OMT_DRM_BUFFERS 3
/* A flip that has not completed in this long means the display is gone. */
#define OMT_FLIP_TIMEOUT_MS 500u
/* Decode threads. Every supported board is quad-core, and the audio worker
 * needs one of those cores. */
#define OMT_DECODE_WORKERS 3
/* How close a mode's refresh has to be to the rate asked for. */
#define OMT_RATE_TOLERANCE 0.02
/* Undecodable frames held over in a row before the session is rebuilt. */
#define OMT_SKIP_BUDGET 20u

typedef enum {
    OMT_PRESENTED,
    /* Frame-local VMX damage on a configuration that has already flipped,
     * inside the skip budget; the on-screen buffer is left alone. */
    OMT_PRESENT_SKIPPED,
    OMT_PRESENT_UNSUPPORTED,
    OMT_PRESENT_FAILED,
} omt_present;

typedef struct {
    uint32_t handle;
    uint32_t pitch;
    uint64_t size;
    uint32_t framebuffer;
    uint8_t *map; /* mapped once, for the life of the surface */
} omt_surface;

typedef struct {
    int32_t width, height, frame_rate_n, frame_rate_d, color_space;
} omt_video_format;

typedef struct {
    bool active;
    uint32_t crtc;
    omt_surface surfaces[OMT_DRM_BUFFERS];
    size_t surface_count;
    size_t front;
    bool flip_pending;
    vmx_decoder *decoder;
    bool scaled;
    omt_scaler scaler;
    vmx_placement placed; /* the scaler's tables, as the decoder takes them */
    omt_video_format format;
    char progressive_detail[256];
    char interlaced_detail[256];
    bool presented;
    uint32_t skips;
} omt_drm_config;

typedef struct {
    int fd;
    uint32_t connector_id;
    omt_drm_config cfg;
    bool has_rejected;
    omt_video_format rejected;
    char rejected_detail[512];
    omt_video_ceiling ceiling;
} omt_video_output;

OMT_NODISCARD bool omt_video_open(omt_video_output *o, const char *device, uint32_t connector_id,
                                  const omt_video_ceiling *ceiling, omt_err *err);
void omt_video_close(omt_video_output *o);
/* Decodes and displays one frame; `detail` receives the reason for anything
 * but OMT_PRESENTED/SKIPPED. */
omt_present omt_video_present(omt_video_output *o, const omt_frame *frame, char *detail,
                              size_t detail_size);
const char *omt_video_presentation_detail(const omt_video_output *o, bool interlaced);

/* Mode selection over what a mode means, so it can be tested without DRM. */
typedef struct {
    uint16_t width, height;
    double refresh;
    bool interlaced;
} omt_mode_shape;
typedef struct {
    size_t index;
    bool scaled;
} omt_mode_choice;
bool omt_choose_mode(const omt_mode_shape *shapes, size_t count, uint16_t width, uint16_t height,
                     double requested, omt_mode_choice *out);
omt_present omt_classify_decode(vmx_status error, bool presented, uint32_t *skips, char *detail,
                                size_t size);
void omt_describe_presentation(bool interlaced, size_t src_w, size_t src_h, bool scaled,
                               size_t mode_w, size_t mode_h, char *out, size_t size);

#endif
