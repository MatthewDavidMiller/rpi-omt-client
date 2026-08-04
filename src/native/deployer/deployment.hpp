#pragma once

#include "core.hpp"
#include "process.hpp"

#include <functional>
#include <string>
#include <string_view>

namespace omt::deployer {

struct RemoteResult;

class DeploymentService final {
public:
    using Event = std::function<void(std::string_view)>;

    explicit DeploymentService(std::string version, Event event = {}, StopRequested stop = {});
    ~DeploymentService();
    void install_prerequisites(const std::filesystem::path& project_root);
    void test_connection(const Connection& connection);
    void deploy(const Connection& connection, const Options& options);
    [[nodiscard]] std::string status(const Connection& connection,
                                     std::string_view remote_directory);
    [[nodiscard]] std::string logs(const Connection& connection,
                                   std::string_view remote_directory);
    [[nodiscard]] std::string restart(const Connection& connection,
                                      std::string_view remote_directory);
    void apply_wifi(const Connection& connection, const WifiSettings& settings);

private:
    [[nodiscard]] std::string redact(std::string_view message) const;
    void replace_secrets(std::vector<std::string> secrets);
    void emit(std::string_view message) const;
    void checkpoint() const;
    void require_success(const ProcessResult& result, std::string_view operation) const;
    void require_success(const RemoteResult& result, std::string_view operation) const;
    void require_platform(const RemoteResult& result) const;
    [[nodiscard]] std::string manage(const Connection& connection,
                                     std::string_view remote_directory,
                                     std::string_view action);

    std::string version_;
    Event event_;
    StopRequested stop_;
    mutable std::vector<std::string> secrets_;
};

}  // namespace omt::deployer
