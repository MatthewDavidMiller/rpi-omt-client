# Cross-compile the deployment application for 64-bit Windows from Linux.
#
# The appliance itself never runs Windows code; only the operator-facing
# deployer does. Building it here keeps a single Linux workstation able to
# produce both published deployer packages from one source tree.

set(CMAKE_SYSTEM_NAME Windows)
set(CMAKE_SYSTEM_PROCESSOR x86_64)

set(OMT_MINGW_TRIPLE "x86_64-w64-mingw32")

find_program(OMT_MINGW_C_COMPILER "${OMT_MINGW_TRIPLE}-gcc")
find_program(OMT_MINGW_RC_COMPILER "${OMT_MINGW_TRIPLE}-windres")
if(NOT OMT_MINGW_C_COMPILER)
    message(FATAL_ERROR
        "The ${OMT_MINGW_TRIPLE} cross toolchain is missing. Run: make install")
endif()

set(CMAKE_C_COMPILER "${OMT_MINGW_C_COMPILER}")
find_program(OMT_MINGW_STRIP "${OMT_MINGW_TRIPLE}-strip")
if(OMT_MINGW_STRIP)
    set(CMAKE_STRIP "${OMT_MINGW_STRIP}")
endif()
if(OMT_MINGW_RC_COMPILER)
    set(CMAKE_RC_COMPILER "${OMT_MINGW_RC_COMPILER}")
endif()

# Distributions disagree about where the cross sysroot lives: Fedora/RHEL nest
# it under sys-root/mingw, Debian and Arch do not. Search whichever exists so
# find_library/find_path resolve Windows libraries rather than host ones.
foreach(candidate
        "/usr/${OMT_MINGW_TRIPLE}/sys-root/mingw"
        "/usr/${OMT_MINGW_TRIPLE}")
    if(IS_DIRECTORY "${candidate}")
        list(APPEND CMAKE_FIND_ROOT_PATH "${candidate}")
    endif()
endforeach()
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE ONLY)

# The published .exe has to start on a stock Windows host with no runtime
# installed alongside it, so the GCC support library is linked statically.
set(OMT_MINGW_STATIC_RUNTIME "-static -static-libgcc")
foreach(link_kind EXE SHARED MODULE)
    set(CMAKE_${link_kind}_LINKER_FLAGS_INIT "${OMT_MINGW_STATIC_RUNTIME}")
endforeach()
