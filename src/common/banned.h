/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Force-included into every translation unit (mk/flags.mk passes -include).
 *
 * The Rust workspace could not call an unbounded string routine because none
 * existed. Here they exist, so they are poisoned: any later use is a compile
 * error rather than a review finding. The headers that declare them are pulled
 * in first, because poisoning an identifier a system header still has to
 * declare would fail the build on the declaration itself.
 *
 * The C library generators (rand, srand) are refused by scripts/check-c.sh
 * rather than poisoned here, because OpenSSL's headers use `rand` as a
 * parameter name.
 */
#ifndef OMT_BANNED_H
#define OMT_BANNED_H

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#ifndef _WIN32
#include <alloca.h>
#endif

#pragma GCC poison strcpy strcat strncpy strncat sprintf vsprintf gets
#pragma GCC poison strtok atoi atol atoll tmpnam mktemp
/* alsa-lib's headers name alloca in macros the receiver never expands, so the
 * one file that includes them (mk/targets.mk sets OMT_ALSA_HEADERS for it) is
 * spared this poison; -Wvla and review cover it there. */
#ifndef OMT_ALSA_HEADERS
#undef alloca
#pragma GCC poison alloca
#endif

#endif
