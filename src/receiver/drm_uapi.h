/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The slice of the Linux DRM kernel ABI the receiver uses: legacy and atomic
 * mode-setting, planes and their properties, dumb buffers, and page-flip
 * events. These are declared here
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
#define OMT_IOW(nr, type)                                                                          \
    ((unsigned long)((OMT_IOC_WRITE << 30) | ((unsigned)sizeof(type) << 16) |                      \
                     ((unsigned)'d' << 8) | (unsigned)(nr)))

#define OMT_DRM_CAP_DUMB_BUFFER 0x1u
#define OMT_DRM_MODE_PAGE_FLIP_EVENT 0x01u
#define OMT_DRM_MODE_FLAG_INTERLACE (1u << 4)
#define OMT_DRM_EVENT_FLIP_COMPLETE 0x02u
#define OMT_DRM_MODE_CONNECTED 1u

#define OMT_DRM_CLIENT_CAP_UNIVERSAL_PLANES 2u
#define OMT_DRM_CLIENT_CAP_ATOMIC 3u
#define OMT_DRM_MODE_ATOMIC_TEST_ONLY 0x0100u
#define OMT_DRM_MODE_ATOMIC_NONBLOCK 0x0200u
#define OMT_DRM_MODE_ATOMIC_ALLOW_MODESET 0x0400u
#define OMT_DRM_MODE_OBJECT_CRTC 0xccccccccu
#define OMT_DRM_MODE_OBJECT_CONNECTOR 0xc0c0c0c0u
#define OMT_DRM_MODE_OBJECT_PLANE 0xeeeeeeeeu
#define OMT_DRM_MODE_PROP_ENUM (1u << 3)
#define OMT_DRM_PLANE_TYPE_PRIMARY 1u
/* fourcc_code('Y', 'U', '1', '6'): three-plane YCbCr 4:2:2. */
#define OMT_DRM_FORMAT_YUV422 0x36315559u

struct omt_drm_set_client_cap {
    uint64_t capability;
    uint64_t value;
};

struct omt_drm_mode_card_res {
    uint64_t fb_id_ptr;
    uint64_t crtc_id_ptr;
    uint64_t connector_id_ptr;
    uint64_t encoder_id_ptr;
    uint32_t count_fbs;
    uint32_t count_crtcs;
    uint32_t count_connectors;
    uint32_t count_encoders;
    uint32_t min_width;
    uint32_t max_width;
    uint32_t min_height;
    uint32_t max_height;
};

struct omt_drm_mode_get_plane_res {
    uint64_t plane_id_ptr;
    uint32_t count_planes;
    uint32_t pad;
};

struct omt_drm_mode_get_plane {
    uint32_t plane_id;
    uint32_t crtc_id;
    uint32_t fb_id;
    uint32_t possible_crtcs;
    uint32_t gamma_size;
    uint32_t count_format_types;
    uint64_t format_type_ptr;
};

struct omt_drm_mode_obj_get_properties {
    uint64_t props_ptr;
    uint64_t prop_values_ptr;
    uint32_t count_props;
    uint32_t obj_id;
    uint32_t obj_type;
    uint32_t pad;
};

struct omt_drm_mode_property_enum {
    uint64_t value;
    char name[32];
};

struct omt_drm_mode_get_property {
    uint64_t values_ptr;
    uint64_t enum_blob_ptr;
    uint32_t prop_id;
    uint32_t flags;
    char name[32];
    uint32_t count_values;
    uint32_t count_enum_blobs;
};

struct omt_drm_mode_create_blob {
    uint64_t data;
    uint32_t length;
    uint32_t blob_id;
};

struct omt_drm_mode_destroy_blob {
    uint32_t blob_id;
};

struct omt_drm_mode_atomic {
    uint32_t flags;
    uint32_t count_objs;
    uint64_t objs_ptr;
    uint64_t count_props_ptr;
    uint64_t props_ptr;
    uint64_t prop_values_ptr;
    uint64_t reserved;
    uint64_t user_data;
};

struct omt_drm_mode_fb_cmd2 {
    uint32_t fb_id;
    uint32_t width;
    uint32_t height;
    uint32_t pixel_format;
    uint32_t flags;
    uint32_t handles[4];
    uint32_t pitches[4];
    uint32_t offsets[4];
    uint32_t pad;
    uint64_t modifier[4];
};

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
_Static_assert(sizeof(struct omt_drm_set_client_cap) == 16, "drm_set_client_cap");
_Static_assert(sizeof(struct omt_drm_mode_card_res) == 64, "drm_mode_card_res");
_Static_assert(sizeof(struct omt_drm_mode_get_plane_res) == 16, "drm_mode_get_plane_res");
_Static_assert(sizeof(struct omt_drm_mode_get_plane) == 32, "drm_mode_get_plane");
_Static_assert(sizeof(struct omt_drm_mode_obj_get_properties) == 32, "drm_mode_obj_get_properties");
_Static_assert(sizeof(struct omt_drm_mode_property_enum) == 40, "drm_mode_property_enum");
_Static_assert(sizeof(struct omt_drm_mode_get_property) == 64, "drm_mode_get_property");
_Static_assert(sizeof(struct omt_drm_mode_create_blob) == 16, "drm_mode_create_blob");
_Static_assert(sizeof(struct omt_drm_mode_destroy_blob) == 4, "drm_mode_destroy_blob");
_Static_assert(sizeof(struct omt_drm_mode_atomic) == 56, "drm_mode_atomic");
_Static_assert(sizeof(struct omt_drm_mode_fb_cmd2) == 104, "drm_mode_fb_cmd2");

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
#define OMT_DRM_IOCTL_SET_CLIENT_CAP OMT_IOW(0x0d, struct omt_drm_set_client_cap)
#define OMT_DRM_IOCTL_MODE_GETRESOURCES OMT_IOWR(0xA0, struct omt_drm_mode_card_res)
#define OMT_DRM_IOCTL_MODE_GETPROPERTY OMT_IOWR(0xAA, struct omt_drm_mode_get_property)
#define OMT_DRM_IOCTL_MODE_GETPLANERESOURCES OMT_IOWR(0xB5, struct omt_drm_mode_get_plane_res)
#define OMT_DRM_IOCTL_MODE_GETPLANE OMT_IOWR(0xB6, struct omt_drm_mode_get_plane)
#define OMT_DRM_IOCTL_MODE_ADDFB2 OMT_IOWR(0xB8, struct omt_drm_mode_fb_cmd2)
#define OMT_DRM_IOCTL_MODE_OBJ_GETPROPERTIES OMT_IOWR(0xB9, struct omt_drm_mode_obj_get_properties)
#define OMT_DRM_IOCTL_MODE_ATOMIC OMT_IOWR(0xBC, struct omt_drm_mode_atomic)
#define OMT_DRM_IOCTL_MODE_CREATEPROPBLOB OMT_IOWR(0xBD, struct omt_drm_mode_create_blob)
#define OMT_DRM_IOCTL_MODE_DESTROYPROPBLOB OMT_IOWR(0xBE, struct omt_drm_mode_destroy_blob)

#endif
