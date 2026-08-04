add_library(omt_deployer_core STATIC
    core.cpp
    process.cpp
    sha256.cpp)
target_include_directories(omt_deployer_core PUBLIC ${CMAKE_CURRENT_SOURCE_DIR})
target_link_libraries(omt_deployer_core PUBLIC omt_warnings omt_hardening omt_sanitizers)
if(WIN32)
    target_link_libraries(omt_deployer_core PRIVATE bcrypt)
endif()

option(OMT_DEPLOYER_GUI "Build the SDL3/ImGui deployment application" ON)
if(OMT_DEPLOYER_GUI)
    include(${PROJECT_SOURCE_DIR}/cmake/NativeDependencies.cmake)
    omt_fetch_deployer_dependencies()
    add_executable(rpi-omt-deployer WIN32
        ui_main.cpp
        deployment.cpp
        ssh_client.cpp)
    target_compile_definitions(rpi-omt-deployer PRIVATE
        OMT_CLIENT_VERSION="${OMT_CLIENT_VERSION}")
    target_link_libraries(rpi-omt-deployer PRIVATE
        omt_deployer_core
        omt_imgui
        SDL3::SDL3-static
        libssh2_static
        omt_warnings
        omt_hardening
        omt_sanitizers)
    if(WIN32)
        target_link_libraries(rpi-omt-deployer PRIVATE ws2_32 crypt32 bcrypt)
    endif()
    set_target_properties(rpi-omt-deployer PROPERTIES
        INTERPROCEDURAL_OPTIMIZATION_RELEASE YES)
    install(TARGETS rpi-omt-deployer
        RUNTIME DESTINATION bin
        COMPONENT Deployer)
endif()
