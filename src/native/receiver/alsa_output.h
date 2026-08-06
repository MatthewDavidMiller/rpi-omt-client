// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#ifndef OMT_RECEIVER_ALSA_OUTPUT_H
#define OMT_RECEIVER_ALSA_OUTPUT_H

#include "native_types.h"

typedef struct omt_alsa_output omt_alsa_output;

omt_alsa_output *omt_alsa_output_create(void);
void omt_alsa_output_destroy(omt_alsa_output *output);
bool omt_alsa_output_write(omt_alsa_output *output, const omt_frame *frame, const char *device,
                           char *error, size_t error_capacity);
void omt_alsa_output_close(omt_alsa_output *output);

#endif
