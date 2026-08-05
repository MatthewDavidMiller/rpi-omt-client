add_library(omt_deployer_core STATIC
    core.cpp
    process.cpp
    sha256.cpp)
target_include_directories(omt_deployer_core PUBLIC ${CMAKE_CURRENT_SOURCE_DIR})
target_link_libraries(omt_deployer_core PUBLIC omt_warnings omt_hardening omt_sanitizers)
if(WIN32)
    target_link_libraries(omt_deployer_core PRIVATE bcrypt)
endif()

# The legal texts are compiled in rather than read from disk, so the About page
# cannot depend on files travelling with the executable. Regenerating on any
# change to the texts keeps the binary and the repository in agreement.
set(OMT_LEGAL_TEXTS
    "${PROJECT_SOURCE_DIR}/LICENSE"
    "${PROJECT_SOURCE_DIR}/THIRD_PARTY_NOTICES.txt")
set(OMT_LEGAL_SOURCE "${CMAKE_CURRENT_BINARY_DIR}/legal_texts.cpp")
add_custom_command(
    OUTPUT "${OMT_LEGAL_SOURCE}"
    COMMAND ${CMAKE_COMMAND}
        -DOMT_EMBED_OUTPUT=${OMT_LEGAL_SOURCE}
        "-DOMT_EMBED_INPUTS=${OMT_LEGAL_TEXTS}"
        -P "${PROJECT_SOURCE_DIR}/cmake/EmbedText.cmake"
    DEPENDS ${OMT_LEGAL_TEXTS}
        "${PROJECT_SOURCE_DIR}/cmake/EmbedText.cmake"
        "${PROJECT_SOURCE_DIR}/cmake/EmbedText.cpp.in"
    COMMENT "Embedding legal texts into the deployer"
    VERBATIM)
add_library(omt_deployer_legal STATIC "${OMT_LEGAL_SOURCE}")
target_include_directories(omt_deployer_legal PUBLIC ${CMAKE_CURRENT_SOURCE_DIR})
target_link_libraries(omt_deployer_legal PUBLIC omt_warnings omt_hardening omt_sanitizers)

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
        omt_deployer_legal
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
