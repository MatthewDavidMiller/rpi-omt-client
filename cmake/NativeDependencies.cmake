include(FetchContent)

# Release archives and hashes are intentionally centralized. Configure may use
# FETCHCONTENT_SOURCE_DIR_* mirrors for offline builds; otherwise CMake rejects
# any archive whose digest differs from this lock.
set(OMT_SDL3_URL
    "https://github.com/libsdl-org/SDL/releases/download/release-3.4.8/SDL3-3.4.8.tar.gz")
set(OMT_SDL3_SHA256
    "e9fff7467fb60f037e6708da18b25560649e4c63edc2a69bb871b960d9cbfbba")
set(OMT_NUKLEAR_URL
    "https://github.com/Immediate-Mode-UI/Nuklear/archive/refs/tags/v4.13.3.tar.gz")
set(OMT_NUKLEAR_SHA256
    "834bf30974a294e996f7b1222aa59f1eb4ee259bd8d7d7967e8a2fb213d82dde")
set(OMT_LIBSSH2_URL
    "https://libssh2.org/download/libssh2-1.11.1.tar.gz")
set(OMT_LIBSSH2_SHA256
    "d9ec76cbe34db98eec3539fe2c899d26b0c837cb3eb466a56b0f109cabf658f7")

# Present a fetched dependency's headers to our targets as system headers, so
# consumers keep -Werror without inheriting upstream warnings.
function(omt_mark_includes_system target)
    if(NOT TARGET ${target})
        return()
    endif()
    get_target_property(include_directories ${target} INTERFACE_INCLUDE_DIRECTORIES)
    if(include_directories)
        set_property(TARGET ${target} APPEND
            PROPERTY INTERFACE_SYSTEM_INCLUDE_DIRECTORIES ${include_directories})
    endif()
endfunction()

function(omt_fetch_deployer_dependencies)
    set(CMAKE_EXPORT_NO_PACKAGE_REGISTRY ON)
    if(POLICY CMP0169)
        cmake_policy(SET CMP0169 OLD)
    endif()
    set(SDL_SHARED OFF CACHE BOOL "" FORCE)
    set(SDL_STATIC ON CACHE BOOL "" FORCE)
    set(SDL_TEST_LIBRARY OFF CACHE BOOL "" FORCE)
    set(SDL_TESTS OFF CACHE BOOL "" FORCE)
    set(SDL_EXAMPLES OFF CACHE BOOL "" FORCE)
    set(SDL_AUDIO OFF CACHE BOOL "" FORCE)
    set(SDL_CAMERA OFF CACHE BOOL "" FORCE)
    set(SDL_GPU OFF CACHE BOOL "" FORCE)
    set(SDL_HAPTIC OFF CACHE BOOL "" FORCE)
    set(SDL_JOYSTICK OFF CACHE BOOL "" FORCE)
    set(SDL_SENSOR OFF CACHE BOOL "" FORCE)
    set(SDL_DIALOG OFF CACHE BOOL "" FORCE)
    set(SDL_HIDAPI OFF CACHE BOOL "" FORCE)
    set(SDL_POWER OFF CACHE BOOL "" FORCE)
    set(SDL_TRAY OFF CACHE BOOL "" FORCE)
    set(SDL_DBUS OFF CACHE BOOL "" FORCE)
    set(SDL_IBUS OFF CACHE BOOL "" FORCE)
    set(SDL_OPENGL OFF CACHE BOOL "" FORCE)
    set(SDL_OPENGLES OFF CACHE BOOL "" FORCE)
    set(SDL_VULKAN OFF CACHE BOOL "" FORCE)
    set(SDL_INSTALL OFF CACHE BOOL "" FORCE)
    if(UNIX AND NOT APPLE)
        set(SDL_WAYLAND OFF CACHE BOOL "" FORCE)
        set(SDL_X11 ON CACHE BOOL "" FORCE)
        foreach(feature XCURSOR XDBE XFIXES XINPUT XRANDR XSCRNSAVER XSHAPE XSYNC XTEST)
            set(SDL_X11_${feature} OFF CACHE BOOL "" FORCE)
        endforeach()
    endif()
    FetchContent_Declare(sdl3
        URL "${OMT_SDL3_URL}"
        URL_HASH "SHA256=${OMT_SDL3_SHA256}"
        DOWNLOAD_EXTRACT_TIMESTAMP TRUE)
    FetchContent_GetProperties(sdl3)
    if(NOT sdl3_POPULATED)
        FetchContent_Populate(sdl3)
    endif()

    # SDL is a C API, but its source archive carries optional platform C++
    # backends. The deployer does not use GameInput, GDK, N-Gage, HID test GUI,
    # or Xbox renderers. Strip those translation units before SDL is added and
    # compile the Windows GameInput no-support shim as C. The archive is pinned,
    # so these exact edits fail closed when upstream layout changes.
    set(sdl_cmake "${sdl3_SOURCE_DIR}/CMakeLists.txt")
    file(READ "${sdl_cmake}" sdl_cmake_text)
    if(WIN32)
        set(gameinput_cpp
            "${sdl3_SOURCE_DIR}/src/video/windows/SDL_windowsgameinput.cpp")
        set(gameinput_c
            "${sdl3_SOURCE_DIR}/src/video/windows/SDL_windowsgameinput.c")
        if(EXISTS "${gameinput_cpp}")
            file(RENAME "${gameinput_cpp}" "${gameinput_c}")
        elseif(NOT EXISTS "${gameinput_c}")
            file(WRITE "${gameinput_c}" [=[
/* C-only no-support shim for the deployer's unused Windows GameInput path. */
#include "SDL_internal.h"
#include "SDL_windowsvideo.h"
bool WIN_InitGameInput(SDL_VideoDevice *device)
{
    (void)device;
    return SDL_Unsupported();
}
bool WIN_UpdateGameInputEnabled(SDL_VideoDevice *device)
{
    (void)device;
    return SDL_Unsupported();
}
void WIN_UpdateGameInput(SDL_VideoDevice *device)
{
    (void)device;
}
void WIN_QuitGameInput(SDL_VideoDevice *device)
{
    (void)device;
}
]=])
        endif()
        string(REPLACE
            "elseif(WINDOWS)\n  enable_language(CXX)\n"
            "elseif(WINDOWS)\n"
            sdl_cmake_text "${sdl_cmake_text}")
        string(REPLACE
            "  check_c_source_compiles(\"\n    #include <stdbool.h>\n    #define COBJMACROS\n    #include <gameinput.h>\n    int main(int argc, char **argv) { return 0; }\" HAVE_GAMEINPUT_H\n  )\n"
            "  set(HAVE_GAMEINPUT_H 0)\n"
            sdl_cmake_text "${sdl_cmake_text}")
        string(REPLACE
            "    \"\${SDL3_SOURCE_DIR}/src/core/windows/*.cpp\"\n"
            ""
            sdl_cmake_text "${sdl_cmake_text}")
        string(REPLACE
            "      \"\${SDL3_SOURCE_DIR}/src/video/windows/*.cpp\"\n"
            ""
            sdl_cmake_text "${sdl_cmake_text}")
    endif()
    file(GLOB_RECURSE sdl_cpp_sources
        "${sdl3_SOURCE_DIR}/*.cc"
        "${sdl3_SOURCE_DIR}/*.cpp"
        "${sdl3_SOURCE_DIR}/*.cxx"
        "${sdl3_SOURCE_DIR}/*.c++"
        "${sdl3_SOURCE_DIR}/*.hpp"
        "${sdl3_SOURCE_DIR}/*.hh"
        "${sdl3_SOURCE_DIR}/*.hxx")
    if(sdl_cpp_sources)
        file(REMOVE ${sdl_cpp_sources})
    endif()
    file(WRITE "${sdl_cmake}" "${sdl_cmake_text}")
    add_subdirectory("${sdl3_SOURCE_DIR}" "${sdl3_BINARY_DIR}")

    set(BUILD_STATIC_LIBS ON CACHE BOOL "" FORCE)
    set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
    set(BUILD_EXAMPLES OFF CACHE BOOL "" FORCE)
    set(BUILD_TESTING OFF CACHE BOOL "" FORCE)
    set(ENABLE_ZLIB_COMPRESSION OFF CACHE BOOL "" FORCE)
    set(ENABLE_DEBUG_LOGGING OFF CACHE BOOL "" FORCE)
    if(WIN32)
        set(CRYPTO_BACKEND WinCNG CACHE STRING "" FORCE)
    else()
        set(CRYPTO_BACKEND OpenSSL CACHE STRING "" FORCE)
    endif()
    FetchContent_Declare(libssh2
        URL "${OMT_LIBSSH2_URL}"
        URL_HASH "SHA256=${OMT_LIBSSH2_SHA256}"
        DOWNLOAD_EXTRACT_TIMESTAMP TRUE)
    FetchContent_MakeAvailable(libssh2)
    file(GLOB_RECURSE libssh2_cpp_sources
        "${libssh2_SOURCE_DIR}/*.cc"
        "${libssh2_SOURCE_DIR}/*.cpp"
        "${libssh2_SOURCE_DIR}/*.cxx"
        "${libssh2_SOURCE_DIR}/*.c++"
        "${libssh2_SOURCE_DIR}/*.hpp"
        "${libssh2_SOURCE_DIR}/*.hh"
        "${libssh2_SOURCE_DIR}/*.hxx")
    if(libssh2_cpp_sources)
        file(REMOVE ${libssh2_cpp_sources})
    endif()
    if(UNIX AND TARGET libssh2_static)
        target_compile_definitions(libssh2_static PRIVATE _DEFAULT_SOURCE)
    endif()

    FetchContent_Declare(nuklear
        URL "${OMT_NUKLEAR_URL}"
        URL_HASH "SHA256=${OMT_NUKLEAR_SHA256}"
        DOWNLOAD_EXTRACT_TIMESTAMP TRUE)
    FetchContent_GetProperties(nuklear)
    if(NOT nuklear_POPULATED)
        FetchContent_Populate(nuklear)
    endif()
    file(GLOB_RECURSE nuklear_cpp_sources
        "${nuklear_SOURCE_DIR}/*.cc"
        "${nuklear_SOURCE_DIR}/*.cpp"
        "${nuklear_SOURCE_DIR}/*.cxx"
        "${nuklear_SOURCE_DIR}/*.c++"
        "${nuklear_SOURCE_DIR}/*.hpp"
        "${nuklear_SOURCE_DIR}/*.hh"
        "${nuklear_SOURCE_DIR}/*.hxx")
    if(nuklear_cpp_sources)
        file(REMOVE ${nuklear_cpp_sources})
    endif()
    add_library(omt_nuklear INTERFACE)
    target_include_directories(omt_nuklear SYSTEM INTERFACE
        "${nuklear_SOURCE_DIR}"
        "${nuklear_SOURCE_DIR}/demo/sdl3_renderer")
    omt_mark_includes_system(SDL3-static)
    omt_mark_includes_system(libssh2_static)
    set(OMT_NUKLEAR_LICENSE "${nuklear_SOURCE_DIR}/LICENSE" PARENT_SCOPE)
endfunction()
