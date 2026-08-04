#pragma once

#include <filesystem>
#include <functional>
#include <string>
#include <vector>

namespace omt::deployer {

struct ProcessResult {
    int exit_code{-1};
    std::string output;
};

using Progress = std::function<void(std::string_view)>;
using StopRequested = std::function<bool()>;

[[nodiscard]] ProcessResult run_process(const std::vector<std::string>& arguments,
                                        const std::filesystem::path& working_directory,
                                        const Progress& progress = {},
                                        const StopRequested& stop_requested = {});

}  // namespace omt::deployer
