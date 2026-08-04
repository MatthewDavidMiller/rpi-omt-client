/* Copyright (c) 2026 Matthew David Miller; SPDX-License-Identifier: MIT */
#include "omt/omt_wire.h"

#include <string.h>

int main(int argc, char **argv)
{
    if (argc != 3) {
        return 2;
    }
    if (strcmp(argv[1], "source") == 0) {
        return omt_is_valid_source_name_utf8(argv[2]) ? 0 : 1;
    }
    if (strcmp(argv[1], "direct") == 0) {
        omt_direct_target target;
        return omt_parse_direct_target(argv[2], &target) ? 0 : 1;
    }
    return 2;
}
