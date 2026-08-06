// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#ifndef OMT_RECEIVER_OMT_CHANNEL_H
#define OMT_RECEIVER_OMT_CHANNEL_H

#include "native_types.h"

typedef struct omt_channel omt_channel;

omt_channel *omt_channel_create(void);
void omt_channel_destroy(omt_channel *channel);
bool omt_channel_connect(omt_channel *channel, const omt_endpoint *endpoint,
                         omt_frame_type subscription, omt_deadline deadline,
                         char *error, size_t error_capacity);
bool omt_channel_receive(omt_channel *channel, omt_frame *frame, omt_deadline deadline,
                         char *error, size_t error_capacity);
void omt_channel_close(omt_channel *channel);
bool omt_channel_connected(const omt_channel *channel);

#endif
