#pragma once

#include <filesystem>
#include <string>

namespace omt::deployer {

[[nodiscard]] std::string sha256_file(const std::filesystem::path& path);

}  // namespace omt::deployer
