/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * OMT frame, video, and audio headers, and the direct-target and source-name
 * grammars: everything a network peer or an operator can hand the protocol
 * module.
 */
#include "protocol/omt.h"

int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size);

int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size) {
    omt_frame_header h;
    if (omt_parse_frame_header(data, size, &h, NULL) && size > OMT_HEADER_SIZE) {
        omt_video_header v;
        omt_audio_header a;
        (void)!omt_parse_video_header(&h, data + OMT_HEADER_SIZE, size - OMT_HEADER_SIZE, &v, NULL);
        (void)!omt_parse_audio_header(&h, data + OMT_HEADER_SIZE, size - OMT_HEADER_SIZE, &a, NULL);
    }
    omt_direct_target t;
    (void)!omt_parse_direct_target((const char *)data, size, &t, NULL);
    (void)omt_is_valid_source_name((const char *)data, size);
    return 0;
}
