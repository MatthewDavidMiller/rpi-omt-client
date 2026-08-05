// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#include "json_text.hpp"
#include "playback_status.hpp"

#include <chrono>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>
#include <thread>
#include <unistd.h>

namespace {

using omt::native::append_json_string;
using omt::native::json_string;
using omt::native::sanitize_status_detail;

void require(bool value, const char* message)
{
    if (!value) {
        std::cerr << "FAIL: " << message << '\n';
        std::exit(1);
    }
}

std::string read_file(const std::filesystem::path& path)
{
    std::ifstream input(path, std::ios::binary);
    return {std::istreambuf_iterator<char>(input), std::istreambuf_iterator<char>()};
}

std::size_t count_entries(const std::filesystem::path& directory)
{
    std::size_t entries = 0u;
    for (const auto& entry : std::filesystem::directory_iterator(directory)) {
        (void)entry;
        ++entries;
    }
    return entries;
}

/// The status detail is the one field carrying text the receiver did not
/// author: a peer's error string, a `strerror` result, a Discovery Server
/// message. The Web consumer decodes the record with a strict UTF-8 JSON parser
/// that rejects the whole document on a single bad byte, which would pin the
/// dashboard to "Playback status stale" with no way to see why.
void detail_sanitization_contract()
{
    require(sanitize_status_detail("Playing OMT video.") == "Playing OMT video.",
            "clean ASCII detail is preserved");
    require(sanitize_status_detail("  padded\t\r\n") == "padded",
            "surrounding whitespace is trimmed");
    require(sanitize_status_detail(" \t\r\n").empty(), "an all-whitespace detail becomes empty");
    require(sanitize_status_detail("").empty(), "an empty detail stays empty");
    require(sanitize_status_detail("a\nb") == "ab", "interior controls are dropped, not escaped");
    require(sanitize_status_detail("a\x7f" "b") == "ab", "DEL is dropped");

    // Valid multi-byte scalars survive; every malformed encoding is dropped
    // byte by byte rather than passed through or replaced.
    require(sanitize_status_detail("Caf\xc3\xa9") == "Caf\xc3\xa9", "valid UTF-8 is preserved");
    // Only the byte that failed is dropped; parsing resumes at the next one, so
    // a printable byte that merely followed a bad lead is still real text.
    require(sanitize_status_detail("a\xc3\x28" "b") == "a(b", "parsing resynchronizes after a bad lead");
    require(sanitize_status_detail("a\xc3\xc3" "b") == "ab", "a bad continuation byte is dropped");
    require(sanitize_status_detail("a\xc0\xaf" "b") == "ab", "an overlong encoding is dropped");
    require(sanitize_status_detail("a\xed\xa0\x80" "b") == "ab", "a surrogate is dropped");
    require(sanitize_status_detail("a\xf5\x80\x80\x80" "b") == "ab", "an above-U+10FFFF scalar is dropped");
    require(sanitize_status_detail("a\xff" "b") == "ab", "a byte that starts no sequence is dropped");
    require(sanitize_status_detail("a\xe2\x82") == "a", "a truncated tail sequence is dropped");

    // The cap is a byte budget, and a scalar is never split across it.
    const std::string oversized(4096u, 'x');
    require(sanitize_status_detail(oversized).size() == 2048u, "an oversized detail is truncated");
    const std::string wide = std::string(2047u, 'x') + "\xe2\x82\xac";
    const std::string truncated = sanitize_status_detail(wide);
    require(truncated.size() == 2047u, "a scalar that would cross the cap is dropped whole");
}

/// Both JSON producers -- the `discover`/`probe` output and the published status
/// -- go through this one escaper, so its contract is what keeps the Web
/// consumer's strict decoder from rejecting a document the receiver wrote.
void json_escaping_contract()
{
    require(json_string("plain") == "\"plain\"", "plain text is quoted");
    require(json_string("a\"b") == "\"a\\\"b\"", "a quote is escaped");
    require(json_string("a\\b") == "\"a\\\\b\"", "a backslash is escaped");
    require(json_string("\b\f\n\r\t") == "\"\\b\\f\\n\\r\\t\"", "the short escapes are used");
    require(json_string("\x01\x1f") == "\"\\u0001\\u001f\"", "other C0 controls use \\u00xx");
    require(json_string("Caf\xc3\xa9") == "\"Caf\xc3\xa9\"", "UTF-8 passes through unescaped");
    require(json_string("\x7f") == "\"\x7f\"", "DEL is not a JSON escape");

    std::string appended = "[";
    append_json_string(appended, "one");
    appended += ',';
    append_json_string(appended, "two");
    appended += ']';
    require(appended == "[\"one\",\"two\"]", "appending composes without clearing the buffer");
}

/// Publication is an atomic replacement through a uniquely named private stage.
/// A stage left behind on any path accumulates in `$OMT_RUNTIME_DIR`, which is
/// a size-capped tmpfs the receiver rewrites for as long as playback runs.
void publication_contract(const std::filesystem::path& directory)
{
    const auto status_path = directory / "playback-status.json";
    const omt::native::Connector connector{
        "HDMI-A-1", "/dev/dri/card1", "/sys/class/drm/card1-HDMI-A-1", 32u,
        "plughw:CARD=vc4hdmi0,DEV=0"};
    omt::native::PlaybackStatus status(status_path.string(), "Camera");
    status.video_running("Playing OMT video.\n", &connector);
    status.audio_failed("Audio\nunavailable", &connector);
    std::string document = read_file(status_path);
    require(document.find("\"schema\":1") != std::string::npos, "schema is published");
    require(document.find("\"state\":\"degraded\"") != std::string::npos, "degraded state is projected");
    require(document.find("\"video_state\":\"running\"") != std::string::npos, "video remains running");
    require(document.find("\"audio_state\":\"failed\"") != std::string::npos, "audio failure is published");
    require(document.find("Audiounavailable") != std::string::npos, "detail controls are removed");
    require(document.find("\"connector\":\"HDMI-A-1\"") != std::string::npos, "connector is published");
    require(document.find("\"drm_device\":\"/dev/dri/card1\"") != std::string::npos,
            "the DRM device is published");
    require(document.find("\"alsa_device\":\"plughw:CARD=vc4hdmi0,DEV=0\"") != std::string::npos,
            "the ALSA device is published");
    require(document.find("\"target\":\"Camera\"") != std::string::npos, "the target is published");
    require(document.find("\"updated_at\":") != std::string::npos, "timestamp is published");

    // An unchanged event inside the heartbeat window reuses the published
    // projection instead of rewriting the file. Without this the status file is
    // replaced once per decoded frame.
    const std::string before = read_file(status_path);
    for (int repeat = 0; repeat < 32; ++repeat) {
        status.video_running("Playing OMT video.\n", &connector);
    }
    require(read_file(status_path) == before, "an unchanged event does not republish");

    // A change publishes immediately, without waiting for the heartbeat.
    status.video_retrying("Waiting for video frames.", &connector);
    document = read_file(status_path);
    require(document.find("\"video_state\":\"retrying\"") != std::string::npos,
            "a changed event publishes immediately");

    // Past the heartbeat interval an unchanged event republishes, so a consumer
    // applying a staleness threshold keeps seeing a fresh record.
    const std::string idle = read_file(status_path);
    std::this_thread::sleep_for(std::chrono::milliseconds(600));
    status.heartbeat(&connector);
    require(read_file(status_path) != idle, "the heartbeat republishes an unchanged projection");

    status.stopped("Playback stopped.", nullptr);
    document = read_file(status_path);
    require(document.find("\"state\":\"stopped\"") != std::string::npos, "terminal state is forced");
    require(document.find("\"connector\":\"none\"") != std::string::npos,
            "an absent connector is named rather than omitted");
    require(count_entries(directory) == 1u, "publication leaves no uncommitted stage behind");
}

} // namespace

int main()
{
    const auto directory = std::filesystem::temp_directory_path() /
        ("omt-native-status-" + std::to_string(static_cast<long>(::getpid())));
    std::filesystem::create_directory(directory);
    detail_sanitization_contract();
    json_escaping_contract();
    publication_contract(directory);
    std::filesystem::remove_all(directory);
    std::cout << "native playback status contracts passed\n";
    return 0;
}
