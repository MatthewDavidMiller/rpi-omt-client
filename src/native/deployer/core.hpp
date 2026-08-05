#pragma once

#include <cstdint>
#include <filesystem>
#include <string>
#include <string_view>
#include <vector>

namespace omt::deployer {

enum class AuthMethod { password, key };

struct Connection {
    std::string host;
    std::string username;
    std::uint16_t port{22};
    AuthMethod auth{AuthMethod::password};
    std::string password;
    std::filesystem::path key_path;
    std::string key_passphrase;
    std::string sudo_password;

    ~Connection() noexcept;
};

struct Options {
    std::filesystem::path project_root;
    std::string remote_directory{"/opt/omt-client"};
    std::string image_name{"omt-client"};
    std::string tarball_name{"omt-client-arm64.tar.gz"};
    bool build_image{true};

    [[nodiscard]] std::filesystem::path tarball_path() const;
    [[nodiscard]] std::filesystem::path manifest_path() const;
};

struct WifiSettings {
    std::string ssid;
    std::string password;
    bool connect{true};

    ~WifiSettings() noexcept;
};

[[nodiscard]] bool valid_host(std::string_view value) noexcept;
[[nodiscard]] bool valid_username(std::string_view value) noexcept;
[[nodiscard]] bool valid_remote_directory(std::string_view value) noexcept;
[[nodiscard]] std::string wifi_error(const WifiSettings& settings);
[[nodiscard]] std::vector<std::string> validate(const Connection& connection);
[[nodiscard]] std::vector<std::string> validate(const Options& options,
                                                bool require_project = true);
[[nodiscard]] std::string shell_quote(std::string_view value);
[[nodiscard]] std::filesystem::path discover_project_root(
    const std::filesystem::path& executable_directory,
    const std::filesystem::path& working_directory);
[[nodiscard]] std::vector<std::string> load_manifest(const std::filesystem::path& path);
[[nodiscard]] std::string random_token(std::size_t byte_count);
void secure_clear(std::string& value) noexcept;

}  // namespace omt::deployer
