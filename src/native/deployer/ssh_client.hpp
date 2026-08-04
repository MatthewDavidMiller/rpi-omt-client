#pragma once

#include "core.hpp"

#include <cstddef>
#include <filesystem>
#include <functional>
#include <memory>
#include <string>
#include <string_view>

namespace omt::deployer {

struct RemoteResult {
    int exit_code{-1};
    std::string output;
    std::string error;
};

class SshClient final {
public:
    explicit SshClient(const Connection& connection);
    ~SshClient();
    SshClient(const SshClient&) = delete;
    SshClient& operator=(const SshClient&) = delete;
    SshClient(SshClient&&) noexcept;
    SshClient& operator=(SshClient&&) noexcept;

    [[nodiscard]] RemoteResult run(std::string_view command, std::string_view input = {},
                                   const std::function<void(std::string_view)>& progress = {});
    void upload(const std::filesystem::path& local_path, std::string_view remote_path,
                const std::function<void(std::uint64_t, std::uint64_t)>& progress = {});

private:
    struct Implementation;
    std::unique_ptr<Implementation> implementation_;
};

}  // namespace omt::deployer
