// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#pragma once

#include <string>
#include <string_view>

namespace omt::native {

/// Append `value` to `output` as one JSON string literal.
///
/// The receiver emits JSON from two places -- the `discover`/`probe` command
/// output and the published playback status -- and both must escape identically:
/// a source name or an error detail that survived one path but not the other
/// would make the Web consumer's strict decoder reject a document the receiver
/// considered well formed. This is that single escaper.
///
/// Bytes are escaped, not scalars. Callers hand this validated UTF-8 (source
/// names pass `omt_is_valid_source_name_utf8`, details pass
/// `sanitize_status_detail`), so multi-byte sequences pass through unchanged and
/// only C0 controls need the `\u00xx` form.
inline void append_json_string(std::string& output, std::string_view value)
{
    constexpr char hex[] = "0123456789abcdef";
    output.push_back('"');
    for (char raw : value) {
        const auto ch = static_cast<unsigned char>(raw);
        switch (ch) {
        case '"': output += "\\\""; break;
        case '\\': output += "\\\\"; break;
        case '\b': output += "\\b"; break;
        case '\f': output += "\\f"; break;
        case '\n': output += "\\n"; break;
        case '\r': output += "\\r"; break;
        case '\t': output += "\\t"; break;
        default:
            if (ch < 0x20u) {
                output += "\\u00";
                output.push_back(hex[ch >> 4u]);
                output.push_back(hex[ch & 0x0fu]);
            } else {
                output.push_back(raw);
            }
            break;
        }
    }
    output.push_back('"');
}

/// Return `value` as one JSON string literal.
[[nodiscard]] inline std::string json_string(std::string_view value)
{
    std::string output;
    output.reserve(value.size() + 2u);
    append_json_string(output, value);
    return output;
}

} // namespace omt::native
