/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Literal IP address parsing with the exact grammar of Rust's Ipv4Addr and
 * Ipv6Addr FromStr, so a target accepted on one side of a contract is
 * accepted on the other: dotted-quad IPv4 with no leading zeros, and IPv6
 * with at most one "::" and an optional trailing dotted quad. No zone index.
 */
#ifndef OMT_IPADDR_H
#define OMT_IPADDR_H

#include "common/base.h"

OMT_NODISCARD bool omt_parse_ipv4(const char *s, size_t len, uint8_t out[4]);
OMT_NODISCARD bool omt_parse_ipv6(const char *s, size_t len, uint8_t out[16]);

/* Formats an IPv6 address as Rust's Ipv6Addr Display does: RFC 5952
 * lowercase compression of the longest zero run (of two or more groups), and
 * the dotted form for an IPv4-mapped address. out must hold 46 bytes. */
void omt_format_ipv6(const uint8_t a[16], char out[46]);

#endif
