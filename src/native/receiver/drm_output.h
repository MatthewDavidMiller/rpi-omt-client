// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#ifndef OMT_RECEIVER_DRM_OUTPUT_H
#define OMT_RECEIVER_DRM_OUTPUT_H

#include "native_types.h"

typedef enum {
    OMT_PRESENTED,
    OMT_PRESENT_UNSUPPORTED_FORMAT,
    OMT_PRESENT_FAILED
} omt_present_outcome;

typedef struct omt_drm_output omt_drm_output;

bool omt_find_connector(const char *preference, omt_connector *connector);
bool omt_connector_is_connected(const omt_connector *connector);
omt_drm_output *omt_drm_output_create(const omt_connector *connector);
void omt_drm_output_destroy(omt_drm_output *output);
bool omt_drm_output_ready(const omt_drm_output *output);
const char *omt_drm_output_error(const omt_drm_output *output);
omt_present_outcome omt_drm_output_present(omt_drm_output *output, omt_frame *frame);

#endif
