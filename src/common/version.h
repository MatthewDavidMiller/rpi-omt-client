/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The build's version string, stamped into this one object rather than passed
 * to every translation unit, so a gate run that builds with a different
 * version recompiles one file instead of the tree.
 */
#ifndef OMT_VERSION_H
#define OMT_VERSION_H

extern const char omt_version[];

#endif
