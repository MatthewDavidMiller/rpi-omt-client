/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The manifest-v3 capsule, as compiled into this binary.
 *
 * The deployer carries the appliance rather than pointing at it: the build
 * embeds every member of deploy/manifest-v3.txt, the ARM64 image archive
 * included (tools/gen/gen_capsule.py). An operator therefore runs one
 * executable, with no checkout, no archive to copy in beside it, and no way
 * to pair a deployer of one release with host scripts of another.
 *
 * This file is linked only into the deployer binaries and the capsule suite,
 * so the rest of the deployer builds and tests without the image.
 */
#include <string.h>

#include "deploy/core/deploy.h"

_Static_assert(sizeof(size_t) == 8, "the capsule table is laid out for 64-bit targets");
_Static_assert(sizeof(dp_capsule_member) == 24, "the capsule table is three quads per member");

extern const dp_capsule_member dp_capsule_table[];
extern const uint64_t dp_capsule_count;
extern const char dp_license_data[];
extern const uint64_t dp_license_size;
extern const char dp_notices_data[];
extern const uint64_t dp_notices_size;

const dp_capsule_member *dp_capsule_members(size_t *count) {
    *count = (size_t)dp_capsule_count;
    return dp_capsule_table;
}

const dp_capsule_member *dp_capsule_member_named(const char *name) {
    for (size_t i = 0; i < (size_t)dp_capsule_count; i++) {
        if (strcmp(dp_capsule_table[i].name, name) == 0) return &dp_capsule_table[i];
    }
    return NULL;
}

/* Named rather than inferred: the archive is the one member whose handling
 * differs, and a rule like "the member ending in .tar.gz" would quietly pick a
 * second archive if one were ever added. */
const dp_capsule_member *dp_capsule_image(void) { return dp_capsule_member_named(DP_IMAGE_MEMBER); }

const char *dp_license_text(size_t *len) {
    *len = (size_t)dp_license_size;
    return dp_license_data;
}

const char *dp_notices_text(size_t *len) {
    *len = (size_t)dp_notices_size;
    return dp_notices_data;
}
