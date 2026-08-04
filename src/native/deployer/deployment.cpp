#include "deployment.hpp"

#include "sha256.hpp"
#include "ssh_client.hpp"

#include <algorithm>
#include <array>
#include <cctype>
#include <fstream>
#include <map>
#include <set>
#include <stdexcept>

#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#endif

namespace omt::deployer {
namespace {
constexpr std::string_view arm_check_image =
    "debian:bookworm-slim@sha256:4724b8cc51e33e398f0e2e15e18d5ec2851ff0c2280647e1310bc1642182655d";
constexpr std::string_view binfmt_image =
    "tonistiigi/binfmt@sha256:400a4873b838d1b89194d982c45e5fb3cda4593fbfd7e08a02e76b03b21166f0";
constexpr std::string_view platform_probe =
    "uname -m && . /etc/os-release && printf '%s\\n' \"$ID\" && "
    "cat /etc/alpine-release && tr -d '\\000' < /proc/device-tree/model && printf '\\n'";

void require_regular(const std::filesystem::path& path) {
    std::error_code error;
    if (!std::filesystem::is_regular_file(path, error) ||
        std::filesystem::is_symlink(std::filesystem::symlink_status(path, error))) {
        throw std::runtime_error("Required regular file is missing: " + path.string());
    }
}

void atomic_replace(const std::filesystem::path& stage, const std::filesystem::path& final) {
#ifdef _WIN32
    if (MoveFileExW(stage.c_str(), final.c_str(),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) == 0) {
        throw std::runtime_error("Unable to publish the ARM64 image archive atomically.");
    }
#else
    if (::rename(stage.c_str(), final.c_str()) != 0) {
        throw std::runtime_error("Unable to publish the ARM64 image archive atomically.");
    }
#endif
}

std::string sudo_prefix(const Connection& connection) {
    const bool password = !connection.sudo_password.empty() ||
                          (connection.auth == AuthMethod::password && !connection.password.empty());
    return password ? "sudo -S -p ''" : "sudo -n";
}

std::string sudo_input(const Connection& connection) {
    if (!connection.sudo_password.empty()) {
        return connection.sudo_password + '\n';
    }
    return connection.auth == AuthMethod::password ? connection.password + '\n' : std::string{};
}

std::string trim_digest(std::string_view output) {
    const auto end = output.find_first_of(" \t\r\n");
    std::string digest(output.substr(0, end));
    if (digest.size() != 64 || !std::ranges::all_of(digest, [](const unsigned char c) {
            return std::isxdigit(c) != 0;
        })) {
        return {};
    }
    std::ranges::transform(digest, digest.begin(), [](const unsigned char c) {
        return static_cast<char>(std::tolower(c));
    });
    return digest;
}

}  // namespace

DeploymentService::DeploymentService(std::string version, Event event, StopRequested stop)
    : version_(std::move(version)), event_(std::move(event)), stop_(std::move(stop)) {}

DeploymentService::~DeploymentService() {
    for (auto& secret : secrets_) {
        secure_clear(secret);
    }
}

std::string DeploymentService::redact(const std::string_view message) const {
    std::string safe(message);
    for (const auto& secret : secrets_) {
        if (secret.empty()) {
            continue;
        }
        for (auto offset = safe.find(secret); offset != std::string::npos;
             offset = safe.find(secret, offset + 10)) {
            safe.replace(offset, secret.size(), "[redacted]");
        }
    }
    return safe;
}

void DeploymentService::replace_secrets(std::vector<std::string> secrets) {
    for (auto& secret : secrets_) {
        secure_clear(secret);
    }
    secrets_ = std::move(secrets);
}

void DeploymentService::emit(const std::string_view message) const {
    if (!event_) {
        return;
    }
    event_(redact(message));
}

void DeploymentService::checkpoint() const {
    if (stop_ && stop_()) throw std::runtime_error("Operation cancelled.");
}

void DeploymentService::require_success(const ProcessResult& result,
                                        const std::string_view operation) const {
    if (result.exit_code != 0) {
        throw std::runtime_error(
            redact(std::string(operation) + " failed:\n" + result.output));
    }
}

void DeploymentService::require_success(const RemoteResult& result,
                                        const std::string_view operation) const {
    if (result.exit_code != 0) {
        throw std::runtime_error(
            redact(std::string(operation) + " failed:\n" + result.error + result.output));
    }
}

void DeploymentService::require_platform(const RemoteResult& result) const {
    require_success(result, "Remote platform probe");
    std::array<std::string, 4> lines;
    std::size_t index = 0;
    std::size_t start = 0;
    while (index < lines.size() && start < result.output.size()) {
        const auto end = result.output.find('\n', start);
        lines[index++] = result.output.substr(start, end == std::string::npos
                                                          ? result.output.size() - start
                                                          : end - start);
        start = end == std::string::npos ? result.output.size() : end + 1;
    }
    if (lines[0] != "aarch64" || lines[1] != "alpine" || !lines[2].starts_with("3.23.") ||
        !lines[3].starts_with("Raspberry Pi 5")) {
        throw std::runtime_error(
            "The target must be a Raspberry Pi 5 running Alpine Linux 3.23 aarch64.");
    }
}

void DeploymentService::install_prerequisites(const std::filesystem::path& project_root) {
    checkpoint();
    emit("Installing pinned ARM64 emulation support...\n");
    require_success(run_process({"docker", "run", "--privileged", "--rm",
                                 std::string(binfmt_image), "--install", "arm64"},
                                project_root, [this](auto line) { emit(line); }, stop_),
                    "ARM64 emulator installation");
    require_success(run_process({"docker", "run", "--rm", "--platform", "linux/arm64",
                                 "--entrypoint", "/bin/sh", std::string(arm_check_image), "-c",
                                 "test \"$(uname -m)\" = aarch64"},
                                project_root, [this](auto line) { emit(line); }, stop_),
                    "ARM64 emulator verification");
    checkpoint();
}

void DeploymentService::test_connection(const Connection& connection) {
    checkpoint();
    const auto errors = validate(connection);
    if (!errors.empty()) {
        throw std::runtime_error(errors.front());
    }
    replace_secrets({connection.password, connection.key_passphrase, connection.sudo_password});
    emit("Testing strict SSH connection...\n");
    SshClient remote(connection);
    const auto result = remote.run(platform_probe);
    require_platform(result);
    checkpoint();
    emit("SSH connection and platform checks succeeded.\n");
}

void DeploymentService::deploy(const Connection& connection, const Options& options) {
    checkpoint();
    auto errors = validate(connection);
    const auto option_errors = validate(options);
    errors.insert(errors.end(), option_errors.begin(), option_errors.end());
    if (!errors.empty()) {
        throw std::runtime_error(errors.front());
    }
    replace_secrets({connection.password, connection.key_passphrase, connection.sudo_password});
    const auto manifest_names = load_manifest(options.manifest_path());
    for (const auto& name : manifest_names) {
        if (!(options.build_image && name == options.tarball_name)) {
            require_regular(options.project_root / name);
        }
    }

    if (options.build_image) {
        install_prerequisites(options.project_root);
        emit("Building the ARM64 appliance image...\n");
        const auto stage = options.project_root /
                           ("." + options.tarball_name + "." + random_token(8) + ".tmp");
        try {
            const auto result = run_process(
                {"docker", "buildx", "build", "--platform", "linux/arm64", "--build-arg",
                 "RPI_OMT_CLIENT_VERSION=" + version_, "--output",
                 "type=docker,dest=" + stage.string(), "--file", "deploy/Dockerfile", "-t",
                 options.image_name, "."},
                options.project_root, [this](auto line) { emit(line); }, stop_);
            require_success(result, "ARM64 image build");
            require_regular(stage);
            if (std::filesystem::file_size(stage) < 512) {
                throw std::runtime_error("Docker produced an invalid ARM64 image archive.");
            }
            atomic_replace(stage, options.tarball_path());
        } catch (...) {
            std::error_code ignored;
            std::filesystem::remove(stage, ignored);
            throw;
        }
    } else {
        require_regular(options.tarball_path());
    }

    emit("Connecting and checking the Raspberry Pi...\n");
    checkpoint();
    SshClient remote(connection);
    require_platform(remote.run(platform_probe));
    checkpoint();
    const auto sudo = sudo_prefix(connection);
    require_success(remote.run(sudo + " install -d -m 755 -o \"$(id -u)\" -g \"$(id -g)\" " +
                                   shell_quote(options.remote_directory),
                               sudo_input(connection)),
                    "Remote directory preparation");

    const auto token = random_token(12);
    const auto remote_root = options.remote_directory;
    const auto stage = remote_root + "/.deploy-staging/" + token;
    const auto current_helper = remote_root + "/deploy/transaction.sh";
    const auto legacy_helper = remote_root + "/deploy-transaction.sh";
    require_success(remote.run(
                        "if [ -x " + shell_quote(legacy_helper) + " ] && [ -f " +
                        shell_quote(remote_root + "/deploy-artifacts.txt") + " ]; then " +
                        shell_quote(legacy_helper) + " recover " + shell_quote(remote_root) + " " +
                        shell_quote(remote_root + "/deploy-artifacts.txt") + "; fi; if [ -x " +
                        shell_quote(current_helper) + " ]; then " + shell_quote(current_helper) +
                        " recover " + shell_quote(remote_root) + "; fi"),
                    "Interrupted deployment recovery");
    const auto staging_root = remote_root + "/.deploy-staging";
    require_success(remote.run(
                        "if [ -L " + shell_quote(staging_root) + " ] || { [ -e " +
                        shell_quote(staging_root) + " ] && [ ! -d " +
                        shell_quote(staging_root) + " ]; }; then exit 14; fi; "
                        "install -d -m 700 -- " + shell_quote(staging_root) + "; "
                        "mkdir -- " + shell_quote(stage)),
                    "Remote staging root validation");
    std::set<std::string> directories;
    for (const auto& name : manifest_names) {
        const auto separator = name.find_last_of('/');
        if (separator != std::string::npos) {
            directories.insert(stage + "/" + name.substr(0, separator));
        }
    }
    std::string mkdir{"mkdir -p --"};
    for (const auto& directory : directories) {
        mkdir += " " + shell_quote(directory);
    }
    if (!directories.empty()) {
        require_success(remote.run(mkdir), "Remote staging preparation");
    }

    struct Identity {
        std::uintmax_t size;
        std::filesystem::file_time_type modified;
        std::string digest;
    };
    std::map<std::string, Identity> identities;
    for (const auto& name : manifest_names) {
        const auto local = options.project_root / name;
        identities.emplace(name, Identity{std::filesystem::file_size(local),
                                           std::filesystem::last_write_time(local),
                                           sha256_file(local)});
    }
    try {
        for (const auto& name : manifest_names) {
            checkpoint();
            emit("Uploading " + name + "...\n");
            const auto local = options.project_root / name;
            const auto remote_path = stage + "/" + name;
            remote.upload(local, remote_path, [this](auto, auto) { checkpoint(); });
            const auto& identity = identities.at(name);
            if (std::filesystem::file_size(local) != identity.size ||
                std::filesystem::last_write_time(local) != identity.modified ||
                sha256_file(local) != identity.digest) {
                throw std::runtime_error("Local artifact changed while uploading: " + name);
            }
            const auto checksum = remote.run("sha256sum -- " + shell_quote(remote_path));
            require_success(checksum, "Remote checksum");
            if (trim_digest(checksum.output) != identity.digest) {
                throw std::runtime_error("SHA-256 mismatch after uploading " + name);
            }
        }
        checkpoint();
        const auto manifest = stage + "/deploy/manifest-v3.txt";
        const auto helper = stage + "/deploy/transaction.sh";
        require_success(remote.run("bash " + shell_quote(helper) + " promote " +
                                       shell_quote(remote_root) + " " + shell_quote(token) + " " +
                                       shell_quote(manifest)),
                        "Deployment promotion");
    } catch (...) {
        (void)remote.run("if [ -d " + shell_quote(stage) + " ] && [ ! -L " + shell_quote(stage) +
                         " ]; then find -P " + shell_quote(stage) + " -xdev -depth -delete; fi");
        throw;
    }

    const std::array executable_paths{
        remote_root + "/deploy/host/install.sh", remote_root + "/deploy/host/uninstall.sh",
        remote_root + "/deploy/host/host-diagnostics.sh",
        remote_root + "/deploy/host/host-event-watcher.sh",
        remote_root + "/deploy/host/host-reboot.sh", remote_root + "/deploy/transaction.sh"};
    std::string chmod{"chmod +x"};
    for (const auto& path : executable_paths) {
        chmod += " " + shell_quote(path);
    }
    checkpoint();
    const auto install = "printf 'n\\n' | " + shell_quote(executable_paths.front());
    const auto result = remote.run(chmod + " && " + sudo + " sh -c " + shell_quote(install),
                                   sudo_input(connection), [this](auto line) {
                                       checkpoint();
                                       emit(line);
                                   });
    require_success(result, "Remote installer");
    emit("Deployment completed successfully.\n");
}

std::string DeploymentService::manage(const Connection& connection,
                                      const std::string_view remote_directory,
                                      const std::string_view action) {
    checkpoint();
    if (!validate(connection).empty() || !valid_remote_directory(remote_directory)) {
        throw std::runtime_error("Connection or remote directory is invalid.");
    }
    replace_secrets({connection.password, connection.key_passphrase, connection.sudo_password});
    SshClient remote(connection);
    const auto result = remote.run("cd " + shell_quote(remote_directory) + " && " +
                                   std::string(action));
    require_success(result, "Remote management action");
    return result.output;
}

std::string DeploymentService::status(const Connection& connection,
                                      const std::string_view remote_directory) {
    return manage(connection, remote_directory, "docker compose -f deploy/compose.yml ps");
}
std::string DeploymentService::logs(const Connection& connection,
                                    const std::string_view remote_directory) {
    return manage(connection, remote_directory,
                  "docker compose -f deploy/compose.yml logs --tail=120");
}
std::string DeploymentService::restart(const Connection& connection,
                                       const std::string_view remote_directory) {
    return manage(connection, remote_directory, "docker compose -f deploy/compose.yml restart");
}

void DeploymentService::apply_wifi(const Connection& connection, const WifiSettings& settings) {
    checkpoint();
    if (!validate(connection).empty()) {
        throw std::runtime_error("Connection settings are invalid.");
    }
    if (const auto error = wifi_error(settings); !error.empty()) {
        throw std::runtime_error(error);
    }
    replace_secrets({connection.password, connection.key_passphrase, connection.sudo_password,
                     settings.password});
    SshClient remote(connection);
    const auto marker = "__OMT_WIFI_PASSWORD_" + random_token(12) + "__";
    std::string input = sudo_input(connection) + marker + "\n" + settings.password + "\n";
    const std::string script =
        "marker=$4; while IFS= read -r line; do [ \"$line\" = \"$marker\" ] && break; done; "
        "IFS= read -r pass; ssid=$1; ssid_hex=$2; activate=$3; "
        "raw_psk=$5; command -v wpa_cli >/dev/null; "
        "wpa_cli -i wlan0 ping | grep -Fxq PONG; "
        "if [ \"$raw_psk\" = yes ]; then psk=$(printf '%s' \"$pass\" | tr 'A-F' 'a-f'); "
        "else command -v wpa_passphrase >/dev/null; "
        "psk=$(printf '%s\\n' \"$pass\" | wpa_passphrase \"$ssid\" | sed -n 's/^[[:space:]]*psk=//p' | tail -n1); fi; "
        "unset pass; [ ${#psk} -eq 64 ]; id=; "
        "for candidate in $(wpa_cli -i wlan0 list_networks | awk 'NR>2 {print $1}'); do "
        "[ \"$(wpa_cli -i wlan0 get_network \"$candidate\" ssid 2>/dev/null)\" = \"$ssid_hex\" ] && id=$candidate && break; done; "
        "[ -n \"$id\" ] || id=$(wpa_cli -i wlan0 add_network); case $id in ''|*[!0-9]*) exit 13;; esac; "
        "wpa_cli -i wlan0 set_network \"$id\" ssid \"$ssid_hex\" | grep -Fxq OK; "
        "wpa_cli -i wlan0 set_network \"$id\" key_mgmt WPA-PSK | grep -Fxq OK; "
        "wpa_cli -i wlan0 set_network \"$id\" psk \"$psk\" | grep -Fxq OK; unset psk; "
        "wpa_cli -i wlan0 enable_network \"$id\" | grep -Fxq OK; wpa_cli -i wlan0 save_config | grep -Fxq OK; "
        "[ \"$activate\" = no ] || { wpa_cli -i wlan0 select_network \"$id\" >/dev/null; wpa_cli -i wlan0 reassociate >/dev/null; }";
    std::string ssid_hex;
    constexpr char hex[] = "0123456789abcdef";
    for (const char character : settings.ssid) {
        const auto byte = static_cast<unsigned char>(character);
        ssid_hex += hex[byte >> 4U];
        ssid_hex += hex[byte & 0xFU];
    }
    const bool raw_psk = settings.password.size() == 64U &&
                         std::ranges::all_of(settings.password, [](const unsigned char c) {
                             return (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') ||
                                    (c >= 'A' && c <= 'F');
                         });
    const auto command = sudo_prefix(connection) + " -v && sudo -n sh -eu -c " +
                         shell_quote(script) + " sh " + shell_quote(settings.ssid) + " " +
                         shell_quote(ssid_hex) + " " + (settings.connect ? "yes" : "no") + " " +
                         shell_quote(marker) + " " + (raw_psk ? "yes" : "no");
    RemoteResult result;
    try {
        result = remote.run(command, input, [this](auto line) {
            checkpoint();
            emit(line);
        });
        secure_clear(input);
    } catch (...) {
        secure_clear(input);
        throw;
    }
    require_success(result, "Wi-Fi update");
}

}  // namespace omt::deployer
