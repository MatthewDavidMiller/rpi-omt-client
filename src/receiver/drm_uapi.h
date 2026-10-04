/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The slice of the Linux DRM kernel ABI the receiver uses: legacy
 * mode-setting, dumb buffers, and page-flip events. These are declared here
 * rather than taken from <drm/drm.h> because Alpine's linux-headers package
 * does not ship the DRM uapi, and libdrm's copy would be a dependency for a
 * dozen structs. The layouts are the kernel's stable ABI; every size is
 * asserted below, and tests/c/test_drm_abi.c compares the structs and ioctl
 * numbers against the system's <drm/drm.h> wherever one is installed.
 */
#ifndef OMT_RECEIVER_DRM_UAPI_H
#define OMT_RECEIVER_DRM_UAPI_H

#include <stdint.h>

/* The generic Linux ioctl encoding, which arm64 and x86-64 both use. */
#define OMT_IOC_WRITE 1u
#define OMT_IOC_READ 2u
#define OMT_IOWR(nr, type)                                                                         \
    ((unsigned long)(((OMT_IOC_READ | OMT_IOC_WRITE) << 30) | ((unsigned)sizeof(type) << 16) |     \
                     ((unsigned)'d' << 8) | (unsigned)(nr)))

#define OMT_DRM_CAP_DUMB_BUFFER 0x1u
#define OMT_DRM_MODE_PAGE_FLIP_EVENT 0x01u
#define OMT_DRM_MODE_FLAG_INTERLACE (1u << 4)
#define OMT_DRM_EVENT_FLIP_COMPLETE 0x02u
#define OMT_DRM_MODE_CONNECTED 1u

struct omt_drm_get_cap {
    uint64_t capability;
    uint64_t value;
};

struct omt_drm_mode_modeinfo {
    uint32_t clock;
    uint16_t hdisplay, hsync_start, hsync_end, htotal, hskew;
    uint16_t vdisplay, vsync_start, vsync_end, vtotal, vscan;
    uint32_t vrefresh;
    uint32_t flags;
    uint32_t type;
    char name[32];
};

struct omt_drm_mode_get_connector {
    uint64_t encoders_ptr;
    uint64_t modes_ptr;
    uint64_t props_ptr;
    uint64_t prop_values_ptr;
    uint32_t count_modes;
    uint32_t count_props;
    uint32_t count_encoders;
    uint32_t encoder_id;
    uint32_t connector_id;
    uint32_t connector_type;
    uint32_t connector_type_id;
    uint32_t connection;
    uint32_t mm_width;
    uint32_t mm_height;
    uint32_t subpixel;
    uint32_t pad;
};

struct omt_drm_mode_get_encoder {
    uint32_t encoder_id;
    uint32_t encoder_type;
    uint32_t crtc_id;
    uint32_t possible_crtcs;
    uint32_t possible_clones;
};

struct omt_drm_mode_crtc {
    uint64_t set_connectors_ptr;
    uint32_t count_connectors;
    uint32_t crtc_id;
    uint32_t fb_id;
    uint32_t x;
    uint32_t y;
    uint32_t gamma_size;
    uint32_t mode_valid;
    struct omt_drm_mode_modeinfo mode;
};

struct omt_drm_mode_fb_cmd {
    uint32_t fb_id;
    uint32_t width;
    uint32_t height;
    uint32_t pitch;
    uint32_t bpp;
    uint32_t depth;
    uint32_t handle;
};

struct omt_drm_mode_crtc_page_flip {
    uint32_t crtc_id;
    uint32_t fb_id;
    uint32_t flags;
    uint32_t reserved;
    uint64_t user_data;
};

struct omt_drm_mode_create_dumb {
    uint32_t height;
    uint32_t width;
    uint32_t bpp;
    uint32_t flags;
    uint32_t handle;
    uint32_t pitch;
    uint64_t size;
};

struct omt_drm_mode_map_dumb {
    uint32_t handle;
    uint32_t pad;
    uint64_t offset;
};

struct omt_drm_mode_destroy_dumb {
    uint32_t handle;
};

struct omt_drm_event {
    uint32_t type;
    uint32_t length;
};

_Static_assert(sizeof(struct omt_drm_get_cap) == 16, "drm_get_cap");
_Static_assert(sizeof(struct omt_drm_mode_modeinfo) == 68, "drm_mode_modeinfo");
_Static_assert(sizeof(struct omt_drm_mode_get_connector) == 80, "drm_mode_get_connector");
_Static_assert(sizeof(struct omt_drm_mode_get_encoder) == 20, "drm_mode_get_encoder");
_Static_assert(sizeof(struct omt_drm_mode_crtc) == 104, "drm_mode_crtc");
_Static_assert(sizeof(struct omt_drm_mode_fb_cmd) == 28, "drm_mode_fb_cmd");
_Static_assert(sizeof(struct omt_drm_mode_crtc_page_flip) == 24, "drm_mode_crtc_page_flip");
_Static_assert(sizeof(struct omt_drm_mode_create_dumb) == 32, "drm_mode_create_dumb");
_Static_assert(sizeof(struct omt_drm_mode_map_dumb) == 16, "drm_mode_map_dumb");

#define OMT_DRM_IOCTL_GET_CAP OMT_IOWR(0x0c, struct omt_drm_get_cap)
#define OMT_DRM_IOCTL_MODE_SETCRTC OMT_IOWR(0xA2, struct omt_drm_mode_crtc)
#define OMT_DRM_IOCTL_MODE_GETENCODER OMT_IOWR(0xA6, struct omt_drm_mode_get_encoder)
#define OMT_DRM_IOCTL_MODE_GETCONNECTOR OMT_IOWR(0xA7, struct omt_drm_mode_get_connector)
#define OMT_DRM_IOCTL_MODE_ADDFB OMT_IOWR(0xAE, struct omt_drm_mode_fb_cmd)
#define OMT_DRM_IOCTL_MODE_RMFB OMT_IOWR(0xAF, unsigned int)
#define OMT_DRM_IOCTL_MODE_PAGE_FLIP OMT_IOWR(0xB0, struct omt_drm_mode_crtc_page_flip)
#define OMT_DRM_IOCTL_MODE_CREATE_DUMB OMT_IOWR(0xB2, struct omt_drm_mode_create_dumb)
#define OMT_DRM_IOCTL_MODE_MAP_DUMB OMT_IOWR(0xB3, struct omt_drm_mode_map_dumb)
#define OMT_DRM_IOCTL_MODE_DESTROY_DUMB OMT_IOWR(0xB4, struct omt_drm_mode_destroy_dumb)

#endif
