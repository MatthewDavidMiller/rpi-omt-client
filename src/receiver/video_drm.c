/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "receiver/video_drm.h"

#include <errno.h>
#include <fcntl.h>
#include <math.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>

#include "common/proc.h"
#include "receiver/drm_uapi.h"

#define PLAYING "Playing OMT video."
#define PLAYING_INTERLACED "Playing interlaced input progressively without deinterlacing."

/* glibc declares ioctl's request as unsigned long and musl as int; the
 * kernel reads the same 32 bits either way. */
#ifdef __GLIBC__
typedef unsigned long ioctl_request;
#else
typedef int ioctl_request;
#endif

static int drm_ioctl(int fd, unsigned long request, void *arg) {
    int rc;
    do {
        rc = ioctl(fd, (ioctl_request)request, arg);
    } while (rc == -1 && (errno == EINTR || errno == EAGAIN));
    return rc;
}

/* "<what>: <strerror> (os error N)", the shape the Rust presenter reported. */
static void os_detail(char *detail, size_t size, const char *what, int code) {
    omt_err e;
    omt_err_os(&e, code);
    snprintf(detail, size, "%s: %s", what, e.msg);
}

bool omt_video_open(omt_video_output *o, const char *device, uint32_t connector_id,
                    const omt_video_ceiling *ceiling, omt_err *err) {
    memset(o, 0, sizeof(*o));
    /* Non-blocking, so waiting for a flip event is bounded by poll rather
     * than parked on a read a vanished display will never answer. */
    o->fd = open(device, O_RDWR | O_NONBLOCK | O_CLOEXEC);
    if (o->fd < 0) {
        omt_err e;
        omt_err_os(&e, errno);
        omt_err_set(err, "Failed to open DRM device: %s", e.msg);
        return false;
    }
    struct omt_drm_get_cap cap = {.capability = OMT_DRM_CAP_DUMB_BUFFER};
    if (drm_ioctl(o->fd, OMT_DRM_IOCTL_GET_CAP, &cap) != 0 || cap.value == 0) {
        close(o->fd);
        o->fd = -1;
        omt_err_set(err, "DRM device does not support dumb buffers");
        return false;
    }
    o->connector_id = connector_id;
    o->ceiling = *ceiling;
    return true;
}

static void destroy_surface(int fd, omt_surface *s) {
    if (s->map) munmap(s->map, s->size);
    if (s->framebuffer) {
        uint32_t fb = s->framebuffer;
        (void)drm_ioctl(fd, OMT_DRM_IOCTL_MODE_RMFB, &fb);
    }
    if (s->handle) {
        struct omt_drm_mode_destroy_dumb d = {.handle = s->handle};
        (void)drm_ioctl(fd, OMT_DRM_IOCTL_MODE_DESTROY_DUMB, &d);
    }
    memset(s, 0, sizeof(*s));
}

static bool wait_for_flip(omt_video_output *o, char *detail, size_t size) {
    uint64_t deadline = omt_now_ms() + OMT_FLIP_TIMEOUT_MS;
    for (;;) {
        uint8_t buffer[1024];
        ssize_t n = read(o->fd, buffer, sizeof(buffer));
        if (n > 0) {
            size_t at = 0;
            while ((size_t)n - at >= sizeof(struct omt_drm_event)) {
                struct omt_drm_event ev;
                memcpy(&ev, buffer + at, sizeof(ev));
                if (ev.length < sizeof(ev) || ev.length > (size_t)n - at) break;
                if (ev.type == OMT_DRM_EVENT_FLIP_COMPLETE) return true;
                at += ev.length;
            }
            continue;
        }
        if (n < 0 && errno != EAGAIN && errno != EINTR) {
            os_detail(detail, size, "Unable to wait for DRM page flip", errno);
            return false;
        }
        uint64_t left = omt_remaining_ms(deadline);
        if (left == 0) break;
        struct pollfd p = {o->fd, POLLIN, 0};
        (void)poll(&p, 1, (int)left);
    }
    snprintf(detail, size, "DRM page flip timed out");
    return false;
}

static bool retire_flip(omt_video_output *o, char *detail, size_t size) {
    if (!o->cfg.active || !o->cfg.flip_pending) return true;
    bool ok = wait_for_flip(o, detail, size);
    /* Cleared either way: a flip that timed out is not going to be retired by
     * a later frame, and the session is ending regardless. */
    o->cfg.flip_pending = false;
    return ok;
}

/* Retires any outstanding flip and hands every DRM object back. */
static void release(omt_video_output *o) {
    char ignored[128];
    (void)retire_flip(o, ignored, sizeof(ignored));
    if (!o->cfg.active) return;
    for (size_t i = 0; i < o->cfg.surface_count; i++) destroy_surface(o->fd, &o->cfg.surfaces[i]);
    vmx_decoder_free(o->cfg.decoder);
    if (o->cfg.scaled) omt_scaler_free(&o->cfg.scaler);
    memset(&o->cfg, 0, sizeof(o->cfg));
}

void omt_video_close(omt_video_output *o) {
    if (o->fd < 0) return;
    release(o);
    close(o->fd);
    o->fd = -1;
}

const char *omt_video_presentation_detail(const omt_video_output *o, bool interlaced) {
    if (!o->cfg.active) return PLAYING;
    return interlaced ? o->cfg.interlaced_detail : o->cfg.progressive_detail;
}

void omt_describe_presentation(bool interlaced, size_t src_w, size_t src_h, bool scaled,
                               size_t mode_w, size_t mode_h, char *out, size_t size) {
    const char *base = interlaced ? PLAYING_INTERLACED : PLAYING;
    if (!scaled)
        snprintf(out, size, "%s", base);
    else
        snprintf(out, size, "%s Scaled from %zux%zu to the display's %zux%zu mode.", base, src_w,
                 src_h, mode_w, mode_h);
}

omt_present omt_classify_decode(vmx_status error, bool presented, uint32_t *skips, char *detail,
                                size_t size) {
    switch (error) {
    case VMX_TRUNCATED:
    case VMX_INVALID_FORMAT:
    case VMX_OVERSIZED:
    case VMX_SLICE_COUNT:
    case VMX_CORRUPT_STREAM:
        /* Only damage to this frame's own bitstream can be waited out, and
         * only while a picture is on screen to hold. */
        if (!presented) {
            snprintf(detail, size, "VMX decoder rejected the frame: %s", vmx_status_name(error));
            return OMT_PRESENT_FAILED;
        }
        *skips += 1;
        if (*skips <= OMT_SKIP_BUDGET) return OMT_PRESENT_SKIPPED;
        snprintf(detail, size, "VMX decoder rejected %u consecutive frames: %s", *skips,
                 vmx_status_name(error));
        return OMT_PRESENT_FAILED;
    case VMX_UNSUPPORTED_FORMAT:
        snprintf(detail, size, "VMX decoder rejected the frame: %s", vmx_status_name(error));
        return OMT_PRESENT_UNSUPPORTED;
    case VMX_OK:
    case VMX_EMPTY:
    case VMX_INVALID_DIMENSIONS:
    case VMX_OUTPUT_SIZE:
    case VMX_WORKER_FAILURE: break;
    }
    snprintf(detail, size, "VMX decoder rejected the frame: %s", vmx_status_name(error));
    return OMT_PRESENT_FAILED;
}

static bool usable(const omt_mode_shape *s) {
    return !s->interlaced && s->refresh > 0.0 && s->width <= VMX_MAX_WIDTH &&
           s->height <= VMX_MAX_HEIGHT;
}

/* The best refresh among candidate modes: the rate asked for, its nearest
 * whole rate, then 60 Hz, then the fastest on offer. */
static bool best_rate(const omt_mode_shape *shapes, const size_t *candidates, size_t count,
                      double requested, size_t *out) {
    double expected[3] = {requested, round(requested), 60.0};
    for (int e = 0; e < 3; e++)
        for (size_t i = 0; i < count; i++)
            if (fabs(shapes[candidates[i]].refresh - expected[e]) < OMT_RATE_TOLERANCE) {
                *out = candidates[i];
                return true;
            }
    if (count == 0) return false;
    size_t best = candidates[0];
    for (size_t i = 1; i < count; i++)
        if (shapes[candidates[i]].refresh > shapes[best].refresh) best = candidates[i];
    *out = best;
    return true;
}

bool omt_choose_mode(const omt_mode_shape *shapes, size_t count, uint16_t width, uint16_t height,
                     double requested, omt_mode_choice *out) {
    size_t *usable_list = calloc(count ? count : 1, sizeof(size_t));
    size_t *candidates = calloc(count ? count : 1, sizeof(size_t));
    bool found = false;
    if (!usable_list || !candidates) goto done;
    size_t nu = 0, nc = 0;
    for (size_t i = 0; i < count; i++)
        if (usable(&shapes[i])) usable_list[nu++] = i;
    /* A mode at the video's own size always wins: the decoder can write the
     * scanout buffer directly. */
    for (size_t i = 0; i < nu; i++)
        if (shapes[usable_list[i]].width == width && shapes[usable_list[i]].height == height)
            candidates[nc++] = usable_list[i];
    size_t index;
    if (best_rate(shapes, candidates, nc, requested, &index)) {
        out->index = index;
        out->scaled = false;
        found = true;
        goto done;
    }
    /* Otherwise the largest size the video reduces into, or the smallest on
     * offer when every mode is larger. Ties keep the last of equal area, as
     * max_by_key does, and the first for min_by_key. */
    bool have = false;
    size_t chosen = 0;
    uint32_t chosen_area = 0;
    for (size_t i = 0; i < nu; i++) {
        const omt_mode_shape *s = &shapes[usable_list[i]];
        if (s->width > width || s->height > height) continue;
        uint32_t area = (uint32_t)s->width * s->height;
        if (!have || area >= chosen_area) {
            chosen = usable_list[i];
            chosen_area = area;
            have = true;
        }
    }
    if (!have) {
        for (size_t i = 0; i < nu; i++) {
            const omt_mode_shape *s = &shapes[usable_list[i]];
            uint32_t area = (uint32_t)s->width * s->height;
            if (!have || area < chosen_area) {
                chosen = usable_list[i];
                chosen_area = area;
                have = true;
            }
        }
    }
    if (!have) goto done;
    nc = 0;
    for (size_t i = 0; i < nu; i++)
        if (shapes[usable_list[i]].width == shapes[chosen].width &&
            shapes[usable_list[i]].height == shapes[chosen].height)
            candidates[nc++] = usable_list[i];
    if (best_rate(shapes, candidates, nc, requested, &index)) {
        out->index = index;
        out->scaled = true;
        found = true;
    }
done:
    free(usable_list);
    free(candidates);
    return found;
}

static double refresh_rate(const struct omt_drm_mode_modeinfo *m) {
    if (m->htotal == 0 || m->vtotal == 0) return 0.0;
    return (double)m->clock * 1000.0 / ((double)m->htotal * (double)m->vtotal);
}

/* Reads the connector without forcing a probe: asking with a non-zero mode
 * count returns the current list rather than re-reading the sink's EDID. */
static bool get_connector(int fd, uint32_t id, struct omt_drm_mode_get_connector *info,
                          struct omt_drm_mode_modeinfo **modes, int *code) {
    *modes = NULL;
    for (int attempt = 0; attempt < 4; attempt++) {
        uint32_t capacity = attempt == 0 ? 1 : info->count_modes;
        if (capacity == 0) capacity = 1;
        if (capacity > 1024) capacity = 1024;
        struct omt_drm_mode_modeinfo *list = calloc(capacity, sizeof(*list));
        if (!list) {
            *code = ENOMEM;
            return false;
        }
        memset(info, 0, sizeof(*info));
        info->connector_id = id;
        info->count_modes = capacity;
        info->modes_ptr = (uint64_t)(uintptr_t)list;
        if (drm_ioctl(fd, OMT_DRM_IOCTL_MODE_GETCONNECTOR, info) != 0) {
            *code = errno;
            free(list);
            return false;
        }
        if (info->count_modes <= capacity) {
            *modes = list;
            return true;
        }
        free(list);
    }
    *code = EAGAIN;
    return false;
}

static bool create_surface(int fd, uint16_t width, uint16_t height, omt_surface *s, char *detail,
                           size_t size) {
    memset(s, 0, sizeof(*s));
    struct omt_drm_mode_create_dumb create = {.width = width, .height = height, .bpp = 32};
    if (drm_ioctl(fd, OMT_DRM_IOCTL_MODE_CREATE_DUMB, &create) != 0) {
        os_detail(detail, size, "Unable to create DRM buffer", errno);
        return false;
    }
    s->handle = create.handle;
    s->pitch = create.pitch;
    s->size = create.size;
    struct omt_drm_mode_fb_cmd fb = {.width = width,
                                     .height = height,
                                     .pitch = create.pitch,
                                     .bpp = 32,
                                     .depth = 24,
                                     .handle = create.handle};
    if (drm_ioctl(fd, OMT_DRM_IOCTL_MODE_ADDFB, &fb) != 0) {
        os_detail(detail, size, "Unable to register DRM framebuffer", errno);
        destroy_surface(fd, s);
        return false;
    }
    s->framebuffer = fb.fb_id;
    struct omt_drm_mode_map_dumb map = {.handle = create.handle};
    if (drm_ioctl(fd, OMT_DRM_IOCTL_MODE_MAP_DUMB, &map) != 0) {
        os_detail(detail, size, "Unable to map DRM buffer", errno);
        destroy_surface(fd, s);
        return false;
    }
    void *mapping =
        mmap(NULL, create.size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, (off_t)map.offset);
    if (mapping == MAP_FAILED) {
        os_detail(detail, size, "Unable to map DRM buffer", errno);
        destroy_surface(fd, s);
        return false;
    }
    s->map = mapping;
    return true;
}

/* Selects a mode for the incoming format and rebuilds the flip chain. */
static omt_present configure(omt_video_output *o, const omt_video_header *h, char *detail,
                             size_t size) {
    /* The board's ceiling is checked before anything is released, so an
     * over-ceiling format change leaves a working session intact. */
    double rate = h->frame_rate_d > 0 ? (double)h->frame_rate_n / (double)h->frame_rate_d : 0.0;
    omt_err err;
    if (!omt_ceiling_admits(&o->ceiling, h->width, h->height, rate, &err)) {
        snprintf(detail, size, "%s", err.msg);
        return OMT_PRESENT_UNSUPPORTED;
    }
    /* Releasing the previous chain first keeps peak allocation to one mode's
     * worth of buffers. */
    release(o);

    if (o->connector_id == 0) {
        snprintf(detail, size, "Invalid DRM connector");
        return OMT_PRESENT_FAILED;
    }
    struct omt_drm_mode_get_connector info;
    struct omt_drm_mode_modeinfo *modes = NULL;
    int code = 0;
    if (!get_connector(o->fd, o->connector_id, &info, &modes, &code)) {
        os_detail(detail, size, "Selected HDMI connector is unavailable", code);
        return OMT_PRESENT_FAILED;
    }
    omt_present result = OMT_PRESENT_FAILED;
    omt_drm_config cfg;
    memset(&cfg, 0, sizeof(cfg));
    if (info.connection != OMT_DRM_MODE_CONNECTED) {
        snprintf(detail, size, "Selected HDMI connector is unavailable");
        goto done;
    }
    struct omt_drm_mode_get_encoder encoder = {.encoder_id = info.encoder_id};
    if (info.encoder_id == 0 || drm_ioctl(o->fd, OMT_DRM_IOCTL_MODE_GETENCODER, &encoder) != 0 ||
        encoder.crtc_id == 0) {
        snprintf(detail, size, "Selected HDMI encoder is unavailable");
        goto done;
    }
    cfg.crtc = encoder.crtc_id;

    omt_mode_shape *shapes =
        calloc(info.count_modes ? info.count_modes : 1, sizeof(omt_mode_shape));
    if (!shapes) {
        snprintf(detail, size, "Unable to read the display modes");
        goto done;
    }
    for (uint32_t i = 0; i < info.count_modes; i++) {
        shapes[i].width = modes[i].hdisplay;
        shapes[i].height = modes[i].vdisplay;
        shapes[i].refresh = refresh_rate(&modes[i]);
        shapes[i].interlaced = (modes[i].flags & OMT_DRM_MODE_FLAG_INTERLACE) != 0;
    }
    omt_mode_choice choice;
    double requested = (double)h->frame_rate_n / (double)h->frame_rate_d;
    bool chosen = omt_choose_mode(shapes, info.count_modes, (uint16_t)h->width, (uint16_t)h->height,
                                  requested, &choice);
    free(shapes);
    if (!chosen) {
        snprintf(detail, size, "The display offers no usable mode for %dx%d video.", h->width,
                 h->height);
        result = OMT_PRESENT_UNSUPPORTED;
        goto done;
    }
    struct omt_drm_mode_modeinfo mode = modes[choice.index];
    size_t mode_w = mode.hdisplay, mode_h = mode.vdisplay;
    size_t width = (size_t)h->width, height = (size_t)h->height;

    /* The software side is built before the CRTC changes, so a failure here
     * has no newly active framebuffer to tear down. */
    vmx_status st = vmx_decoder_new(width, height, vmx_color_space_resolve(h->color_space, height),
                                    OMT_DECODE_WORKERS, &cfg.decoder);
    if (st != VMX_OK) {
        snprintf(detail, size, "Unable to create the VMX decoder: %s", vmx_status_name(st));
        result = OMT_PRESENT_UNSUPPORTED;
        goto done;
    }
    if (choice.scaled) {
        omt_placement placement;
        if (!omt_placement_fit(width, height, mode_w, mode_h, &placement)) {
            snprintf(detail, size, "The display's mode cannot carry the OMT video format.");
            result = OMT_PRESENT_UNSUPPORTED;
            goto done;
        }
        if (!omt_scaler_init(&cfg.scaler, width, height, placement, &err)) {
            snprintf(detail, size, "%s", err.msg);
            goto done;
        }
        cfg.scaled = true;
        cfg.placed = (vmx_placement){placement.x,      placement.y,        placement.width,
                                     placement.height, cfg.scaler.columns, cfg.scaler.rows};
    }
    for (size_t i = 0; i < OMT_DRM_BUFFERS; i++) {
        if (!create_surface(o->fd, mode.hdisplay, mode.vdisplay, &cfg.surfaces[i], detail, size))
            goto done;
        cfg.surface_count++;
    }
    /* Letterbox bars stay unwritten for the configuration's life, so they are
     * cleared rather than assumed black. */
    if (cfg.scaled && !omt_placement_fills(&cfg.scaler.placement, mode_w, mode_h))
        for (size_t i = 0; i < cfg.surface_count; i++)
            memset(cfg.surfaces[i].map, 0, cfg.surfaces[i].size);
    uint32_t connector = o->connector_id;
    struct omt_drm_mode_crtc set = {.set_connectors_ptr = (uint64_t)(uintptr_t)&connector,
                                    .count_connectors = 1,
                                    .crtc_id = cfg.crtc,
                                    .fb_id = cfg.surfaces[0].framebuffer,
                                    .mode_valid = 1,
                                    .mode = mode};
    if (drm_ioctl(o->fd, OMT_DRM_IOCTL_MODE_SETCRTC, &set) != 0) {
        os_detail(detail, size, "Unable to set DRM mode", errno);
        goto done;
    }
    cfg.format =
        (omt_video_format){h->width, h->height, h->frame_rate_n, h->frame_rate_d, h->color_space};
    omt_describe_presentation(false, width, height, choice.scaled, mode_w, mode_h,
                              cfg.progressive_detail, sizeof(cfg.progressive_detail));
    omt_describe_presentation(true, width, height, choice.scaled, mode_w, mode_h,
                              cfg.interlaced_detail, sizeof(cfg.interlaced_detail));
    /* SETCRTC scanned out the first surface without a flip. */
    cfg.front = 0;
    cfg.active = true;
    o->cfg = cfg;
    memset(&cfg, 0, sizeof(cfg));
    result = OMT_PRESENTED;
done:
    for (size_t i = 0; i < cfg.surface_count; i++) destroy_surface(o->fd, &cfg.surfaces[i]);
    vmx_decoder_free(cfg.decoder);
    if (cfg.scaled) omt_scaler_free(&cfg.scaler);
    free(modes);
    return result;
}

static bool same_format(const omt_video_format *a, const omt_video_header *h) {
    return a->width == h->width && a->height == h->height && a->frame_rate_n == h->frame_rate_n &&
           a->frame_rate_d == h->frame_rate_d && a->color_space == h->color_space;
}

omt_present omt_video_present(omt_video_output *o, const omt_frame *frame, char *detail,
                              size_t size) {
    detail[0] = 0;
    if (!frame->has_video) {
        snprintf(detail, size, "Unsupported video frame");
        return OMT_PRESENT_UNSUPPORTED;
    }
    const omt_video_header *h = &frame->video;
    if (o->has_rejected) {
        if (same_format(&o->rejected, h)) {
            snprintf(detail, size, "%s", o->rejected_detail);
            return OMT_PRESENT_UNSUPPORTED;
        }
        o->has_rejected = false;
    }
    if (!o->cfg.active || !same_format(&o->cfg.format, h)) {
        omt_present r = configure(o, h, detail, size);
        if (r == OMT_PRESENT_UNSUPPORTED) {
            /* Remembered until the format changes: mode selection reads the
             * whole connector, and the stream keeps arriving. */
            o->has_rejected = true;
            o->rejected = (omt_video_format){h->width, h->height, h->frame_rate_n, h->frame_rate_d,
                                             h->color_space};
            snprintf(o->rejected_detail, sizeof(o->rejected_detail), "%s", detail);
            return r;
        }
        if (r != OMT_PRESENTED) return r;
    }
    const uint8_t *compressed;
    size_t compressed_len;
    if (!omt_frame_media(frame, OMT_VIDEO_HEADER_SIZE, &compressed, &compressed_len)) {
        snprintf(detail, size, "Truncated VMX frame");
        return OMT_PRESENT_FAILED;
    }
    omt_drm_config *c = &o->cfg;
    size_t next = (c->front + 1) % c->surface_count;
    omt_surface *surface = &c->surfaces[next];
    vmx_status st = vmx_decoder_load(c->decoder, compressed, compressed_len);
    if (st != VMX_OK) return omt_classify_decode(st, c->presented, &c->skips, detail, size);
    /* Decoding happens before the outstanding flip is retired: with three
     * surfaces the target is neither on screen nor queued, which is what the
     * third buffer is for. A scaled mode resamples inside the decode, on the
     * whole worker pool, straight into the surface. */
    if (c->scaled)
        st = vmx_decode_bgrx_placed(c->decoder, surface->map, surface->size, surface->pitch,
                                    &c->placed);
    else
        st = vmx_decode_bgrx(c->decoder, surface->map, surface->size, surface->pitch);
    if (st != VMX_OK) return omt_classify_decode(st, c->presented, &c->skips, detail, size);
    if (!retire_flip(o, detail, size)) return OMT_PRESENT_FAILED;
    struct omt_drm_mode_crtc_page_flip flip = {
        .crtc_id = c->crtc, .fb_id = surface->framebuffer, .flags = OMT_DRM_MODE_PAGE_FLIP_EVENT};
    if (drm_ioctl(o->fd, OMT_DRM_IOCTL_MODE_PAGE_FLIP, &flip) != 0) {
        os_detail(detail, size, "Unable to queue DRM page flip", errno);
        return OMT_PRESENT_FAILED;
    }
    c->front = next;
    c->flip_pending = true;
    c->presented = true;
    c->skips = 0;
    return OMT_PRESENTED;
}
