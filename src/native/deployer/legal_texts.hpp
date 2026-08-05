#pragma once

#include <cstddef>
#include <span>
#include <string_view>

namespace omt::deployer {

struct LegalDocument {
    std::string_view name;
    std::string_view text;
};

/// The project licence and third-party notices, compiled into the executable.
///
/// About renders these directly. They are part of the binary rather than files
/// beside it, so a package the operator copied without them still shows the
/// terms it ships under. `cmake/EmbedText.cmake` generates the definition.
[[nodiscard]] std::span<const LegalDocument> legal_documents() noexcept;

}  // namespace omt::deployer
