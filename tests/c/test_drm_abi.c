/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Pins the receiver's own DRM ABI declarations to the kernel's, wherever the
 * build host carries <drm/drm.h>. Alpine's toolbox does not, so there the
 * sizes asserted in drm_uapi.h are the check; a glibc workstation and the
 * Debian-family hosts that carry the headers compare every struct and ioctl.
 */
#include <stddef.h>

#include "receiver/drm_uapi.h"
#include "test.h"

#if defined(__has_include)
#if __has_include(<drm/drm.h>) && __has_include(<drm/drm_mode.h>)
#define OMT_HAVE_SYSTEM_DRM 1
#include <drm/drm.h>
#include <drm/drm_mode.h>
#if __has_include(<drm/drm_fourcc.h>)
#include <drm/drm_fourcc.h>
#endif
#endif
#endif

static void declarations_match_the_kernel(void) {
#ifdef OMT_HAVE_SYSTEM_DRM
    CHECK_INT(OMT_DRM_IOCTL_GET_CAP, DRM_IOCTL_GET_CAP);
    CHECK_INT(OMT_DRM_IOCTL_MODE_SETCRTC, DRM_IOCTL_MODE_SETCRTC);
    CHECK_INT(OMT_DRM_IOCTL_MODE_GETENCODER, DRM_IOCTL_MODE_GETENCODER);
    CHECK_INT(OMT_DRM_IOCTL_MODE_GETCONNECTOR, DRM_IOCTL_MODE_GETCONNECTOR);
    CHECK_INT(OMT_DRM_IOCTL_MODE_ADDFB, DRM_IOCTL_MODE_ADDFB);
    CHECK_INT(OMT_DRM_IOCTL_MODE_RMFB, DRM_IOCTL_MODE_RMFB);
    CHECK_INT(OMT_DRM_IOCTL_MODE_PAGE_FLIP, DRM_IOCTL_MODE_PAGE_FLIP);
    CHECK_INT(OMT_DRM_IOCTL_MODE_CREATE_DUMB, DRM_IOCTL_MODE_CREATE_DUMB);
    CHECK_INT(OMT_DRM_IOCTL_MODE_MAP_DUMB, DRM_IOCTL_MODE_MAP_DUMB);
    CHECK_INT(OMT_DRM_IOCTL_MODE_DESTROY_DUMB, DRM_IOCTL_MODE_DESTROY_DUMB);
    CHECK_INT(OMT_DRM_CAP_DUMB_BUFFER, DRM_CAP_DUMB_BUFFER);
    CHECK_INT(OMT_DRM_MODE_PAGE_FLIP_EVENT, DRM_MODE_PAGE_FLIP_EVENT);
    CHECK_INT(OMT_DRM_MODE_FLAG_INTERLACE, DRM_MODE_FLAG_INTERLACE);
    CHECK_INT(OMT_DRM_EVENT_FLIP_COMPLETE, DRM_EVENT_FLIP_COMPLETE);
    CHECK_INT(sizeof(struct omt_drm_mode_crtc), sizeof(struct drm_mode_crtc));
    CHECK_INT(offsetof(struct omt_drm_mode_crtc, mode), offsetof(struct drm_mode_crtc, mode));
    CHECK_INT(offsetof(struct omt_drm_mode_get_connector, connection),
              offsetof(struct drm_mode_get_connector, connection));
    CHECK_INT(offsetof(struct omt_drm_mode_create_dumb, size),
              offsetof(struct drm_mode_create_dumb, size));
    CHECK_INT(offsetof(struct omt_drm_mode_map_dumb, offset),
              offsetof(struct drm_mode_map_dumb, offset));
    CHECK_INT(offsetof(struct omt_drm_mode_modeinfo, flags),
              offsetof(struct drm_mode_modeinfo, flags));
    CHECK_INT(OMT_DRM_IOCTL_SET_CLIENT_CAP, DRM_IOCTL_SET_CLIENT_CAP);
    CHECK_INT(OMT_DRM_IOCTL_MODE_GETRESOURCES, DRM_IOCTL_MODE_GETRESOURCES);
    CHECK_INT(OMT_DRM_IOCTL_MODE_GETPROPERTY, DRM_IOCTL_MODE_GETPROPERTY);
    CHECK_INT(OMT_DRM_IOCTL_MODE_GETPLANERESOURCES, DRM_IOCTL_MODE_GETPLANERESOURCES);
    CHECK_INT(OMT_DRM_IOCTL_MODE_GETPLANE, DRM_IOCTL_MODE_GETPLANE);
    CHECK_INT(OMT_DRM_IOCTL_MODE_ADDFB2, DRM_IOCTL_MODE_ADDFB2);
    CHECK_INT(OMT_DRM_IOCTL_MODE_OBJ_GETPROPERTIES, DRM_IOCTL_MODE_OBJ_GETPROPERTIES);
    CHECK_INT(OMT_DRM_IOCTL_MODE_ATOMIC, DRM_IOCTL_MODE_ATOMIC);
    CHECK_INT(OMT_DRM_IOCTL_MODE_CREATEPROPBLOB, DRM_IOCTL_MODE_CREATEPROPBLOB);
    CHECK_INT(OMT_DRM_IOCTL_MODE_DESTROYPROPBLOB, DRM_IOCTL_MODE_DESTROYPROPBLOB);
    CHECK_INT(OMT_DRM_CLIENT_CAP_UNIVERSAL_PLANES, DRM_CLIENT_CAP_UNIVERSAL_PLANES);
    CHECK_INT(OMT_DRM_CLIENT_CAP_ATOMIC, DRM_CLIENT_CAP_ATOMIC);
    CHECK_INT(OMT_DRM_MODE_ATOMIC_TEST_ONLY, DRM_MODE_ATOMIC_TEST_ONLY);
    CHECK_INT(OMT_DRM_MODE_ATOMIC_NONBLOCK, DRM_MODE_ATOMIC_NONBLOCK);
    CHECK_INT(OMT_DRM_MODE_ATOMIC_ALLOW_MODESET, DRM_MODE_ATOMIC_ALLOW_MODESET);
    CHECK_INT(OMT_DRM_MODE_OBJECT_CRTC, DRM_MODE_OBJECT_CRTC);
    CHECK_INT(OMT_DRM_MODE_OBJECT_CONNECTOR, DRM_MODE_OBJECT_CONNECTOR);
    CHECK_INT(OMT_DRM_MODE_OBJECT_PLANE, DRM_MODE_OBJECT_PLANE);
    CHECK_INT(OMT_DRM_MODE_PROP_ENUM, DRM_MODE_PROP_ENUM);
    CHECK_INT(sizeof(struct omt_drm_mode_atomic), sizeof(struct drm_mode_atomic));
    CHECK_INT(sizeof(struct omt_drm_mode_fb_cmd2), sizeof(struct drm_mode_fb_cmd2));
    CHECK_INT(offsetof(struct omt_drm_mode_fb_cmd2, modifier),
              offsetof(struct drm_mode_fb_cmd2, modifier));
    CHECK_INT(sizeof(struct omt_drm_mode_get_property), sizeof(struct drm_mode_get_property));
    CHECK_INT(sizeof(struct omt_drm_mode_get_plane), sizeof(struct drm_mode_get_plane));
    CHECK_INT(sizeof(struct omt_drm_mode_obj_get_properties),
              sizeof(struct drm_mode_obj_get_properties));
    CHECK_INT(sizeof(struct omt_drm_mode_card_res), sizeof(struct drm_mode_card_res));
#if defined(__has_include)
#if __has_include(<drm/drm_fourcc.h>)
    CHECK_INT(OMT_DRM_FORMAT_YUV422, DRM_FORMAT_YUV422);
#endif
#endif
#else
    CHECK(sizeof(struct omt_drm_mode_crtc) == 104);
    /* fourcc_code('Y', 'U', '1', '6'), spelled out where the header is not. */
    CHECK_INT(OMT_DRM_FORMAT_YUV422,
              (uint32_t)'Y' | (uint32_t)'U' << 8 | (uint32_t)'1' << 16 | (uint32_t)'6' << 24);
#endif
}

int main(void) {
    RUN(declarations_match_the_kernel);
    return TEST_EXIT();
}
