cmake_minimum_required(VERSION 3.25)

project(rpi_omt_client VERSION 0.1.0 LANGUAGES C CXX)

set(CMAKE_C_STANDARD 17)
set(CMAKE_C_STANDARD_REQUIRED ON)
set(CMAKE_C_EXTENSIONS OFF)
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

option(OMT_BUILD_RECEIVER "Build the Linux OMT receiver" ON)
option(OMT_BUILD_DEPLOYER "Build the portable deployment GUI" OFF)
option(OMT_BUILD_TESTS "Build native unit tests" ON)
option(OMT_ENABLE_SANITIZERS "Enable address and undefined-behavior sanitizers" OFF)

set(OMT_CLIENT_VERSION "unknown" CACHE STRING "Version embedded in native artifacts")

add_library(omt_warnings INTERFACE)
target_compile_options(omt_warnings INTERFACE
    $<$<COMPILE_LANG_AND_ID:C,GNU,Clang>:-Wall;-Wextra;-Wpedantic;-Werror;-Wconversion;-Wsign-conversion;-Wshadow;-Wformat=2;-Wundef;-Wstrict-prototypes>
    $<$<COMPILE_LANG_AND_ID:CXX,GNU,Clang>:-Wall;-Wextra;-Wpedantic;-Werror;-Wconversion;-Wsign-conversion;-Wshadow;-Wformat=2;-Wundef;-Wold-style-cast;-Wnon-virtual-dtor>
    $<$<CXX_COMPILER_ID:MSVC>:/W4;/WX;/permissive-;/sdl>
)

add_library(omt_hardening INTERFACE)
target_compile_options(omt_hardening INTERFACE
    $<$<AND:$<PLATFORM_ID:Linux>,$<CONFIG:Release>>:-U_FORTIFY_SOURCE;-D_FORTIFY_SOURCE=3;-fstack-protector-strong;-fPIE;-fno-plt>
    $<$<AND:$<PLATFORM_ID:Windows>,$<CXX_COMPILER_ID:GNU>,$<CONFIG:Release>>:-fstack-protector-strong>
)
target_link_options(omt_hardening INTERFACE
    $<$<AND:$<PLATFORM_ID:Linux>,$<CONFIG:Release>>:-pie;-Wl,-z,relro,-z,now,-z,noexecstack;-Wl,--as-needed>
    # ASLR, DEP, and 64-bit address-space layout randomization for the
    # cross-compiled deployer, which MinGW does not enable by default.
    $<$<AND:$<PLATFORM_ID:Windows>,$<CXX_COMPILER_ID:GNU>,$<CONFIG:Release>>:-Wl,--dynamicbase;-Wl,--nxcompat;-Wl,--high-entropy-va>
)

if(OMT_ENABLE_SANITIZERS)
    add_library(omt_sanitizers INTERFACE)
    target_compile_options(omt_sanitizers INTERFACE -fsanitize=address,undefined -fno-omit-frame-pointer)
    target_link_options(omt_sanitizers INTERFACE -fsanitize=address,undefined)
else()
    add_library(omt_sanitizers INTERFACE)
endif()

# POSIX sockets and byte-order code that only the appliance receiver links.
# The Windows deployer cross build has no receiver, so it must not require it.
if(OMT_BUILD_RECEIVER OR (OMT_BUILD_TESTS AND UNIX))
    add_subdirectory(src/native/omt)
endif()

if(OMT_BUILD_RECEIVER)
    add_subdirectory(src/native/receiver)
endif()

if(OMT_BUILD_DEPLOYER)
    add_subdirectory(src/native/deployer)
endif()

if(OMT_BUILD_TESTS)
    enable_testing()
    add_subdirectory(tests/native)
endif()
