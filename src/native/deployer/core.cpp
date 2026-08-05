#include "core.hpp"

#include <algorithm>
#include <array>
#include <cctype>
#include <fstream>
#include <stdexcept>
#include <system_error>

#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <bcrypt.h>
#else
#include <cerrno>
#include <sys/random.h>
#endif

namespace omt::deployer {
namespace {

bool ascii_token(std::string_view value, std::string_view extra) noexcept {
    return !value.empty() && std::ranges::all_of(value, [extra](const unsigned char c) {
        return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') ||
               (c >= '0' && c <= '9') ||
               extra.find(static_cast<char>(c)) != std::string_view::npos;
    });
}

bool ascii_hex(const unsigned char c) noexcept {
    return (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F');
}

bool valid_utf8(const std::string_view value) noexcept {
    for (std::size_t offset = 0; offset < value.size();) {
        const auto first = static_cast<unsigned char>(value[offset]);
        std::size_t continuation = 0;
        std::uint32_t scalar = 0;
        std::uint32_t minimum = 0;
        if (first < 0x80U) {
            scalar = first;
        } else if ((first & 0xE0U) == 0xC0U) {
            continuation = 1; scalar = first & 0x1FU; minimum = 0x80U;
        } else if ((first & 0xF0U) == 0xE0U) {
            continuation = 2; scalar = first & 0x0FU; minimum = 0x800U;
        } else if ((first & 0xF8U) == 0xF0U) {
            continuation = 3; scalar = first & 0x07U; minimum = 0x10000U;
        } else {
            return false;
        }
        if (offset + continuation >= value.size()) return false;
        for (std::size_t index = 0; index < continuation; ++index) {
            const auto next = static_cast<unsigned char>(value[offset + index + 1]);
            if ((next & 0xC0U) != 0x80U) return false;
            scalar = (scalar << 6U) | (next & 0x3FU);
        }
        if ((continuation > 0 && scalar < minimum) || scalar > 0x10FFFFU ||
            (scalar >= 0xD800U && scalar <= 0xDFFFU)) {
            return false;
        }
        offset += continuation + 1;
    }
    return true;
}

bool contains_control(const std::string_view value) noexcept {
    return std::ranges::any_of(value, [](const unsigned char c) { return c < 0x20U || c == 0x7FU; });
}

bool valid_manifest_name(std::string_view name) noexcept {
    if (name.empty() || name.size() > 240 || name.front() == '/' || name.back() == '/' ||
        name.find("//") != std::string_view::npos || !ascii_token(name, "._-/")) {
        return false;
    }
    std::size_t start = 0;
    while (start <= name.size()) {
        const auto end = name.find('/', start);
        const auto component = name.substr(start, end == std::string_view::npos ? name.size() - start
                                                                                : end - start);
        if (component.empty() || component == "." || component == "..") {
            return false;
        }
        if (end == std::string_view::npos) {
            break;
        }
        start = end + 1;
    }
    return true;
}

}  // namespace

std::filesystem::path Options::tarball_path() const { return project_root / tarball_name; }
std::filesystem::path Options::manifest_path() const {
    return project_root / "deploy" / "manifest-v3.txt";
}

bool valid_host(const std::string_view value) noexcept {
    if (value.empty() || value.size() > 253 || contains_control(value)) {
        return false;
    }
    std::size_t start = 0;
    while (start <= value.size()) {
        const auto end = value.find('.', start);
        const auto label = value.substr(start, end == std::string_view::npos ? value.size() - start
                                                                            : end - start);
        if (label.empty() || label.size() > 63 || label.front() == '-' || label.back() == '-' ||
            !ascii_token(label, "-")) {
            return false;
        }
        if (end == std::string_view::npos) {
            break;
        }
        start = end + 1;
    }
    return true;
}

bool valid_username(const std::string_view value) noexcept {
    return value.size() <= 64 && !contains_control(value) && ascii_token(value, "._-");
}

bool valid_remote_directory(const std::string_view value) noexcept {
    if (value.size() < 2 || value.size() > 240 || value.front() != '/' || value.back() == '/' ||
        value.find("//") != std::string_view::npos) {
        return false;
    }
    return valid_manifest_name(value.substr(1));
}

std::string wifi_error(const WifiSettings& settings) {
    if (settings.ssid.empty()) {
        return "Wi-Fi SSID is required.";
    }
    if (settings.ssid.size() > 32 || contains_control(settings.ssid) ||
        !valid_utf8(settings.ssid)) {
        return "Wi-Fi SSID must contain at most 32 UTF-8 bytes and no control characters.";
    }
    if (contains_control(settings.password)) {
        return "Wi-Fi password must not contain control characters.";
    }
    const bool hex = settings.password.size() == 64 &&
                     std::ranges::all_of(settings.password, ascii_hex);
    const bool printable = settings.password.size() >= 8 && settings.password.size() <= 63 &&
                           std::ranges::all_of(settings.password, [](const unsigned char c) {
                               return c >= 0x20U && c <= 0x7EU;
                           });
    return hex || printable
               ? std::string{}
               : "Wi-Fi password must be 8-63 printable ASCII characters or a 64-digit hex PSK.";
}

std::vector<std::string> validate(const Connection& connection) {
    constexpr std::size_t max_secret_bytes = 4U * 1024U;
    constexpr std::size_t max_key_path_units = 4U * 1024U;
    std::vector<std::string> errors;
    if (!valid_host(connection.host)) {
        errors.emplace_back("Pi host must be a valid IPv4 address or DNS host name.");
    }
    if (!valid_username(connection.username)) {
        errors.emplace_back("SSH username contains invalid characters.");
    }
    if (connection.port == 0) {
        errors.emplace_back("SSH port must be between 1 and 65535.");
    }
    if (connection.auth == AuthMethod::password && connection.password.empty()) {
        errors.emplace_back("SSH password is required for password authentication.");
    }
    if (connection.auth == AuthMethod::key) {
        std::error_code error;
        if (connection.key_path.empty() || connection.key_path.native().size() > max_key_path_units ||
            !std::filesystem::is_regular_file(connection.key_path, error)) {
            errors.emplace_back("SSH private-key file does not exist.");
        }
    }
    if (connection.password.size() > max_secret_bytes ||
        connection.key_passphrase.size() > max_secret_bytes ||
        connection.sudo_password.size() > max_secret_bytes) {
        errors.emplace_back("Authentication secrets must not exceed 4096 bytes.");
    }
    for (const auto* pair : std::array{
             &connection.password, &connection.key_passphrase, &connection.sudo_password}) {
        if (contains_control(*pair)) {
            errors.emplace_back("Authentication secrets must not contain control characters.");
            break;
        }
    }
    return errors;
}

std::vector<std::string> validate(const Options& options, const bool require_project) {
    std::vector<std::string> errors;
    std::error_code error;
    if (require_project && !std::filesystem::is_directory(options.project_root, error)) {
        errors.emplace_back("Project root does not exist.");
    }
    if (!valid_remote_directory(options.remote_directory)) {
        errors.emplace_back("Remote install directory is not a normalized safe absolute path.");
    }
    if (!ascii_token(options.image_name, "._-:") || !ascii_token(options.tarball_name, "._-")) {
        errors.emplace_back("Image and archive names contain unsafe characters.");
    }
    return errors;
}

std::string shell_quote(const std::string_view value) {
    std::string result{"'"};
    result.reserve(value.size() + 8);
    for (const char c : value) {
        if (c == '\'') {
            result += "'\\''";
        } else {
            result += c;
        }
    }
    result += '\'';
    return result;
}

std::vector<std::string> load_manifest(const std::filesystem::path& path) {
    std::error_code error;
    if (!std::filesystem::is_regular_file(path, error) ||
        std::filesystem::is_symlink(std::filesystem::symlink_status(path, error)) ||
        std::filesystem::file_size(path, error) > 32'768U) {
        throw std::runtime_error("Deployment manifest is missing, unsafe, or oversized.");
    }
    std::ifstream input(path);
    std::string line;
    if (!std::getline(input, line) || line != "version=3") {
        throw std::runtime_error("Deployment manifest must begin with version=3.");
    }
    std::vector<std::string> names;
    while (std::getline(input, line)) {
        if (!valid_manifest_name(line) ||
            std::ranges::find(names, line) != names.end() || names.size() >= 128) {
            throw std::runtime_error("Deployment manifest contains an invalid or duplicate path.");
        }
        names.push_back(line);
    }
    if (names.empty() || std::ranges::find(names, "deploy/transaction.sh") == names.end() ||
        std::ranges::find(names, "deploy/manifest-v3.txt") == names.end()) {
        throw std::runtime_error("Manifest v3 is incomplete.");
    }
    return names;
}

std::string random_token(const std::size_t byte_count) {
    if (byte_count == 0 || byte_count > 64) {
        throw std::invalid_argument("invalid random token length");
    }
    std::array<unsigned char, 64> random{};
#ifdef _WIN32
    if (BCryptGenRandom(nullptr, random.data(), static_cast<ULONG>(byte_count),
                        BCRYPT_USE_SYSTEM_PREFERRED_RNG) != 0) {
        throw std::runtime_error("The operating-system random generator failed.");
    }
#else
    std::size_t offset = 0;
    while (offset < byte_count) {
        const auto count = ::getrandom(random.data() + offset, byte_count - offset, 0);
        if (count < 0) {
            if (errno == EINTR) continue;
            throw std::runtime_error("The operating-system random generator failed.");
        }
        offset += static_cast<std::size_t>(count);
    }
#endif
    constexpr char hex[] = "0123456789abcdef";
    std::string result(byte_count * 2, '0');
    for (std::size_t index = 0; index < byte_count; ++index) {
        const auto value = random[index];
        result[index * 2] = hex[value >> 4U];
        result[index * 2 + 1] = hex[value & 0xFU];
    }
    return result;
}

void secure_clear(std::string& value) noexcept {
    volatile char* memory = value.empty() ? nullptr : value.data();
    for (std::size_t index = 0; index < value.size(); ++index) {
        memory[index] = '\0';
    }
    value.clear();
}

Connection::~Connection() noexcept {
    secure_clear(password);
    secure_clear(key_passphrase);
    secure_clear(sudo_password);
}

WifiSettings::~WifiSettings() noexcept { secure_clear(password); }

}  // namespace omt::deployer
