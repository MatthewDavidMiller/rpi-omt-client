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
    omt_scaler_free(&o->cfg.scaler);
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

/* ------------------------------------------------- YCbCr through the plane */

/* Names of the properties the YCbCr path sets, per object. */
static const char *const CONNECTOR_PROPS[] = {"CRTC_ID"};
static const char *const CRTC_PROPS[] = {"MODE_ID", "ACTIVE"};
static const char *const PLANE_PROPS[] = {"FB_ID",  "CRTC_ID", "SRC_X",          "SRC_Y",
                                          "SRC_W",  "SRC_H",   "CRTC_X",         "CRTC_Y",
                                          "CRTC_W", "CRTC_H",  "COLOR_ENCODING", "COLOR_RANGE"};
#define PLANE_PROP_COUNT (sizeof(PLANE_PROPS) / sizeof(PLANE_PROPS[0]))
#define MAX_OBJECT_PROPS 64u

static bool set_cap(int fd, uint64_t capability) {
    struct omt_drm_set_client_cap cap = {.capability = capability, .value = 1};
    return drm_ioctl(fd, OMT_DRM_IOCTL_SET_CLIENT_CAP, &cap) == 0;
}

/* Fills ids[i] with the id of the property names[i] on one object; false when
 * any is missing. `type_value`, when asked for, receives the plane "type". */
static bool find_props(int fd, uint32_t object, uint32_t object_type, const char *const *names,
                       size_t count, uint32_t *ids, uint64_t *type_value) {
    uint32_t prop_ids[MAX_OBJECT_PROPS];
    uint64_t values[MAX_OBJECT_PROPS];
    struct omt_drm_mode_obj_get_properties get = {.props_ptr = (uint64_t)(uintptr_t)prop_ids,
                                                  .prop_values_ptr = (uint64_t)(uintptr_t)values,
                                                  .count_props = MAX_OBJECT_PROPS,
                                                  .obj_id = object,
                                                  .obj_type = object_type};
    if (drm_ioctl(fd, OMT_DRM_IOCTL_MODE_OBJ_GETPROPERTIES, &get) != 0 ||
        get.count_props > MAX_OBJECT_PROPS)
        return false;
    for (size_t n = 0; n < count; n++) ids[n] = 0;
    for (uint32_t i = 0; i < get.count_props; i++) {
        struct omt_drm_mode_get_property prop = {.prop_id = prop_ids[i]};
        if (drm_ioctl(fd, OMT_DRM_IOCTL_MODE_GETPROPERTY, &prop) != 0) continue;
        prop.name[sizeof(prop.name) - 1] = 0;
        if (type_value && strcmp(prop.name, "type") == 0) *type_value = values[i];
        for (size_t n = 0; n < count; n++)
            if (strcmp(prop.name, names[n]) == 0) ids[n] = prop_ids[i];
    }
    for (size_t n = 0; n < count; n++)
        if (ids[n] == 0) return false;
    return true;
}

/* The value an enum property gives the entry called `name`. */
static bool enum_value(int fd, uint32_t prop_id, const char *name, uint64_t *out) {
    struct omt_drm_mode_property_enum entries[16];
    struct omt_drm_mode_get_property prop = {.prop_id = prop_id};
    if (drm_ioctl(fd, OMT_DRM_IOCTL_MODE_GETPROPERTY, &prop) != 0 ||
        !(prop.flags & OMT_DRM_MODE_PROP_ENUM) || prop.count_enum_blobs > 16)
        return false;
    prop.enum_blob_ptr = (uint64_t)(uintptr_t)entries;
    prop.count_values = 0;
    prop.values_ptr = 0;
    if (drm_ioctl(fd, OMT_DRM_IOCTL_MODE_GETPROPERTY, &prop) != 0) return false;
    for (uint32_t i = 0; i < prop.count_enum_blobs && i < 16; i++) {
        entries[i].name[sizeof(entries[i].name) - 1] = 0;
        if (strcmp(entries[i].name, name) == 0) {
            *out = entries[i].value;
            return true;
        }
    }
    return false;
}

/* The primary plane that can scan YCbCr 4:2:2 out on `crtc`. */
static bool find_yuv_plane(int fd, uint32_t crtc, uint32_t *plane_out, uint32_t *ids) {
    uint32_t crtcs[16];
    struct omt_drm_mode_card_res res = {.crtc_id_ptr = (uint64_t)(uintptr_t)crtcs,
                                        .count_crtcs = 16};
    if (drm_ioctl(fd, OMT_DRM_IOCTL_MODE_GETRESOURCES, &res) != 0 || res.count_crtcs > 16)
        return false;
    uint32_t crtc_bit = 0;
    for (uint32_t i = 0; i < res.count_crtcs; i++)
        if (crtcs[i] == crtc) crtc_bit = 1u << i;
    if (crtc_bit == 0) return false;
    uint32_t planes[64];
    struct omt_drm_mode_get_plane_res pres = {.plane_id_ptr = (uint64_t)(uintptr_t)planes,
                                              .count_planes = 64};
    if (drm_ioctl(fd, OMT_DRM_IOCTL_MODE_GETPLANERESOURCES, &pres) != 0 || pres.count_planes > 64)
        return false;
    for (uint32_t i = 0; i < pres.count_planes; i++) {
        uint32_t formats[128];
        struct omt_drm_mode_get_plane plane = {.plane_id = planes[i],
                                               .count_format_types = 128,
                                               .format_type_ptr = (uint64_t)(uintptr_t)formats};
        if (drm_ioctl(fd, OMT_DRM_IOCTL_MODE_GETPLANE, &plane) != 0 ||
            plane.count_format_types > 128 || !(plane.possible_crtcs & crtc_bit))
            continue;
        bool yuv = false;
        for (uint32_t f = 0; f < plane.count_format_types; f++)
            yuv = yuv || formats[f] == OMT_DRM_FORMAT_YUV422;
        uint64_t type = UINT64_MAX;
        if (!yuv ||
            !find_props(fd, planes[i], OMT_DRM_MODE_OBJECT_PLANE, PLANE_PROPS, PLANE_PROP_COUNT,
                        ids, &type) ||
            type != OMT_DRM_PLANE_TYPE_PRIMARY)
            continue;
        *plane_out = planes[i];
        return true;
    }
    return false;
}

/* One dumb buffer holding all three planes: luma rows at the buffer's pitch,
 * then Cb and Cr at half of it. Filled with YCbCr black, so the modeset that
 * shows it before the first frame shows black rather than green. */
static bool create_yuv_surface(int fd, uint16_t width, uint16_t height, omt_surface *s,
                               char *detail, size_t size) {
    memset(s, 0, sizeof(*s));
    struct omt_drm_mode_create_dumb create = {
        .width = width, .height = (uint32_t)height * 2, .bpp = 8};
    if (drm_ioctl(fd, OMT_DRM_IOCTL_MODE_CREATE_DUMB, &create) != 0) {
        os_detail(detail, size, "Unable to create DRM buffer", errno);
        return false;
    }
    s->handle = create.handle;
    s->pitch = create.pitch;
    s->size = create.size;
    uint32_t luma_bytes = create.pitch * height, chroma_pitch = create.pitch / 2;
    if (create.pitch % 2 != 0 || (uint64_t)luma_bytes * 2 > create.size) {
        snprintf(detail, size, "DRM buffer pitch cannot carry planar YCbCr");
        destroy_surface(fd, s);
        return false;
    }
    s->plane_offsets[0] = 0;
    s->plane_offsets[1] = luma_bytes;
    s->plane_offsets[2] = luma_bytes + chroma_pitch * height;
    s->plane_pitches[0] = create.pitch;
    s->plane_pitches[1] = chroma_pitch;
    s->plane_pitches[2] = chroma_pitch;
    struct omt_drm_mode_fb_cmd2 fb = {
        .width = width, .height = height, .pixel_format = OMT_DRM_FORMAT_YUV422};
    for (int p = 0; p < 3; p++) {
        fb.handles[p] = create.handle;
        fb.pitches[p] = s->plane_pitches[p];
        fb.offsets[p] = s->plane_offsets[p];
    }
    if (drm_ioctl(fd, OMT_DRM_IOCTL_MODE_ADDFB2, &fb) != 0) {
        os_detail(detail, size, "Unable to register a YCbCr framebuffer", errno);
        destroy_surface(fd, s);
        return false;
    }
    s->framebuffer = fb.fb_id;
    struct omt_drm_mode_map_dumb map = {.handle = create.handle};
    void *mapping = MAP_FAILED;
    if (drm_ioctl(fd, OMT_DRM_IOCTL_MODE_MAP_DUMB, &map) == 0)
        mapping =
            mmap(NULL, create.size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, (off_t)map.offset);
    if (mapping == MAP_FAILED) {
        os_detail(detail, size, "Unable to map DRM buffer", errno);
        destroy_surface(fd, s);
        return false;
    }
    s->map = mapping;
    memset(s->map, 16, luma_bytes);
    memset(s->map + luma_bytes, 128, (size_t)chroma_pitch * height * 2);
    return true;
}

/* An atomic request under construction: objects in order, each with its
 * properties. */
typedef struct {
    uint32_t objects[3];
    uint32_t counts[3];
    uint32_t props[24];
    uint64_t values[24];
    uint32_t object_count;
    uint32_t prop_count;
} atomic_request;

static void request_object(atomic_request *r, uint32_t object) {
    r->objects[r->object_count] = object;
    r->counts[r->object_count++] = 0;
}

static void request_prop(atomic_request *r, uint32_t prop, uint64_t value) {
    r->props[r->prop_count] = prop;
    r->values[r->prop_count++] = value;
    r->counts[r->object_count - 1]++;
}

static int request_commit(int fd, const atomic_request *r, uint32_t flags) {
    struct omt_drm_mode_atomic atomic = {.flags = flags,
                                         .count_objs = r->object_count,
                                         .objs_ptr = (uint64_t)(uintptr_t)r->objects,
                                         .count_props_ptr = (uint64_t)(uintptr_t)r->counts,
                                         .props_ptr = (uint64_t)(uintptr_t)r->props,
                                         .prop_values_ptr = (uint64_t)(uintptr_t)r->values};
    return drm_ioctl(fd, OMT_DRM_IOCTL_MODE_ATOMIC, &atomic);
}

/* Brings the mode up with the first YCbCr surface on the primary plane, the
 * picture fitted into `placement` by the display hardware. False, with the
 * reason in `detail`, when anything refuses; the caller then takes the BGRX
 * path, so nothing this leaves behind is a failure of the session. */
static bool configure_yuv(int fd, uint32_t connector, omt_drm_config *cfg,
                          const struct omt_drm_mode_modeinfo *mode, size_t width, size_t height,
                          vmx_color_space color_space, const omt_placement *placement, char *detail,
                          size_t size) {
    uint32_t plane_ids[PLANE_PROP_COUNT], connector_ids[1], crtc_ids[2];
    uint64_t encoding = 0, range = 0;
    if (!set_cap(fd, OMT_DRM_CLIENT_CAP_UNIVERSAL_PLANES) ||
        !set_cap(fd, OMT_DRM_CLIENT_CAP_ATOMIC)) {
        snprintf(detail, size, "the DRM device offers no atomic mode-setting");
        return false;
    }
    if (!find_yuv_plane(fd, cfg->crtc, &cfg->plane, plane_ids) ||
        !find_props(fd, connector, OMT_DRM_MODE_OBJECT_CONNECTOR, CONNECTOR_PROPS, 1, connector_ids,
                    NULL) ||
        !find_props(fd, cfg->crtc, OMT_DRM_MODE_OBJECT_CRTC, CRTC_PROPS, 2, crtc_ids, NULL)) {
        snprintf(detail, size, "no primary plane on this display takes planar YCbCr 4:2:2");
        return false;
    }
    if (!enum_value(fd, plane_ids[10],
                    color_space == VMX_BT601 ? "ITU-R BT.601 YCbCr" : "ITU-R BT.709 YCbCr",
                    &encoding) ||
        !enum_value(fd, plane_ids[11], "YCbCr limited range", &range)) {
        snprintf(detail, size, "the display plane offers no matching YCbCr colour encoding");
        return false;
    }
    /* At the video's own size, nearest-neighbour keeps chroma repeated
     * across each pixel pair, as the decoder's own conversion does: the
     * picture matches it within rounding. A scaled mode takes the hardware's
     * default filter, which is smoother than the nearest-neighbour resample
     * it replaces. Kernels without the property get their own default. */
    const char *filter_name[] = {"SCALING_FILTER"};
    uint32_t filter_prop = 0;
    uint64_t filter = 0;
    bool has_filter =
        find_props(fd, cfg->plane, OMT_DRM_MODE_OBJECT_PLANE, filter_name, 1, &filter_prop, NULL) &&
        enum_value(fd, filter_prop, cfg->scaled ? "Default" : "Nearest Neighbor", &filter);
    for (size_t i = 0; i < OMT_DRM_BUFFERS; i++) {
        if (!create_yuv_surface(fd, (uint16_t)width, (uint16_t)height, &cfg->surfaces[i], detail,
                                size))
            return false;
        cfg->surface_count++;
    }
    struct omt_drm_mode_create_blob blob = {.data = (uint64_t)(uintptr_t)mode,
                                            .length = sizeof(*mode)};
    if (drm_ioctl(fd, OMT_DRM_IOCTL_MODE_CREATEPROPBLOB, &blob) != 0) {
        os_detail(detail, size, "Unable to describe the DRM mode", errno);
        return false;
    }
    atomic_request r;
    memset(&r, 0, sizeof(r));
    request_object(&r, connector);
    request_prop(&r, connector_ids[0], cfg->crtc);
    request_object(&r, cfg->crtc);
    request_prop(&r, crtc_ids[0], blob.blob_id);
    request_prop(&r, crtc_ids[1], 1);
    request_object(&r, cfg->plane);
    uint64_t plane_values[PLANE_PROP_COUNT] = {cfg->surfaces[0].framebuffer,
                                               cfg->crtc,
                                               0,
                                               0,
                                               (uint64_t)width << 16,
                                               (uint64_t)height << 16,
                                               placement->x,
                                               placement->y,
                                               placement->width,
                                               placement->height,
                                               encoding,
                                               range};
    for (size_t i = 0; i < PLANE_PROP_COUNT; i++) request_prop(&r, plane_ids[i], plane_values[i]);
    if (has_filter) request_prop(&r, filter_prop, filter);
    /* Asked first, so a refusal leaves the display exactly as it was. */
    bool ok = request_commit(
                  fd, &r, OMT_DRM_MODE_ATOMIC_TEST_ONLY | OMT_DRM_MODE_ATOMIC_ALLOW_MODESET) == 0;
    if (!ok)
        os_detail(detail, size, "the display hardware refused YCbCr scanout", errno);
    else if (request_commit(fd, &r, OMT_DRM_MODE_ATOMIC_ALLOW_MODESET) != 0) {
        os_detail(detail, size, "Unable to set DRM mode for YCbCr scanout", errno);
        ok = false;
    }
    /* The committed state holds its own reference to the mode. */
    struct omt_drm_mode_destroy_blob destroy = {.blob_id = blob.blob_id};
    (void)drm_ioctl(fd, OMT_DRM_IOCTL_MODE_DESTROYPROPBLOB, &destroy);
    if (!ok) return false;
    cfg->plane_fb_prop = plane_ids[0];
    cfg->yuv = true;
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
    omt_placement placement = {0, 0, mode_w, mode_h};
    if (choice.scaled && !omt_placement_fit(width, height, mode_w, mode_h, &placement)) {
        snprintf(detail, size, "The display's mode cannot carry the OMT video format.");
        result = OMT_PRESENT_UNSUPPORTED;
        goto done;
    }
    cfg.scaled = choice.scaled;
    char why[256];
    if (configure_yuv(o->fd, o->connector_id, &cfg, &mode, width, height,
                      vmx_color_space_resolve(h->color_space, height), &placement, why,
                      sizeof(why))) {
        fprintf(stderr, "Display path: planar YCbCr 4:2:2, converted%s by the display hardware.\n",
                choice.scaled ? " and scaled" : "");
    } else {
        /* Whatever the YCbCr attempt built goes before the fallback builds
         * its own, so peak allocation stays one chain. */
        for (size_t i = 0; i < cfg.surface_count; i++) destroy_surface(o->fd, &cfg.surfaces[i]);
        cfg.surface_count = 0;
        cfg.plane = 0;
        fprintf(stderr, "Display path: BGRX converted by the decoder (%s).\n", why);
        if (choice.scaled) {
            if (!omt_scaler_init(&cfg.scaler, width, height, placement, &err)) {
                snprintf(detail, size, "%s", err.msg);
                goto done;
            }
            cfg.placed = (vmx_placement){placement.x,      placement.y,        placement.width,
                                         placement.height, cfg.scaler.columns, cfg.scaler.rows};
        }
        for (size_t i = 0; i < OMT_DRM_BUFFERS; i++) {
            if (!create_surface(o->fd, mode.hdisplay, mode.vdisplay, &cfg.surfaces[i], detail,
                                size))
                goto done;
            cfg.surface_count++;
        }
        /* Letterbox bars stay unwritten for the configuration's life, so they
         * are cleared rather than assumed black. */
        if (cfg.scaled && !omt_placement_fills(&placement, mode_w, mode_h))
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
    omt_scaler_free(&cfg.scaler);
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
     * third buffer is for. YCbCr is copied out plane by plane and converted
     * and scaled by the display; the BGRX fallback converts in the decoder,
     * and a scaled mode resamples there too, on the whole worker pool. */
    if (c->yuv) {
        vmx_plane planes[3];
        for (int p = 0; p < 3; p++) {
            uint64_t end = p < 2 ? surface->plane_offsets[p + 1] : surface->size;
            planes[p] =
                (vmx_plane){surface->map + surface->plane_offsets[p],
                            (size_t)(end - surface->plane_offsets[p]), surface->plane_pitches[p]};
        }
        st = vmx_decode_yuv422p(c->decoder, planes);
    } else if (c->scaled) {
        st = vmx_decode_bgrx_placed(c->decoder, surface->map, surface->size, surface->pitch,
                                    &c->placed);
    } else {
        st = vmx_decode_bgrx(c->decoder, surface->map, surface->size, surface->pitch);
    }
    if (st != VMX_OK) return omt_classify_decode(st, c->presented, &c->skips, detail, size);
    if (!retire_flip(o, detail, size)) return OMT_PRESENT_FAILED;
    if (c->yuv) {
        /* A flip is the plane's framebuffer and nothing else. */
        atomic_request r;
        memset(&r, 0, sizeof(r));
        request_object(&r, c->plane);
        request_prop(&r, c->plane_fb_prop, surface->framebuffer);
        if (request_commit(o->fd, &r,
                           OMT_DRM_MODE_ATOMIC_NONBLOCK | OMT_DRM_MODE_PAGE_FLIP_EVENT) != 0) {
            os_detail(detail, size, "Unable to queue DRM page flip", errno);
            return OMT_PRESENT_FAILED;
        }
    } else {
        struct omt_drm_mode_crtc_page_flip flip = {.crtc_id = c->crtc,
                                                   .fb_id = surface->framebuffer,
                                                   .flags = OMT_DRM_MODE_PAGE_FLIP_EVENT};
        if (drm_ioctl(o->fd, OMT_DRM_IOCTL_MODE_PAGE_FLIP, &flip) != 0) {
            os_detail(detail, size, "Unable to queue DRM page flip", errno);
            return OMT_PRESENT_FAILED;
        }
    }
    c->front = next;
    c->flip_pending = true;
    c->presented = true;
    c->skips = 0;
    return OMT_PRESENTED;
}
