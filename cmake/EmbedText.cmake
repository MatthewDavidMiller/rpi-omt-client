# Compile text files into a C17 translation unit.
#
# The deployer ships as a single executable that operators copy wherever they
# like, so its legal texts cannot be files it has to find again at run time:
# a missing or moved file would leave the About page with nothing to show.
# The bytes are emitted verbatim as a numeric array, which keeps the result
# independent of source encoding, embedded quotes, and raw-string delimiters.
#
# Usage:
#   cmake -DOMT_EMBED_OUTPUT=<file.c> -DOMT_EMBED_INPUTS=<a;b>
#         -P cmake/EmbedText.cmake

if(NOT DEFINED OMT_EMBED_OUTPUT OR NOT DEFINED OMT_EMBED_INPUTS)
    message(FATAL_ERROR "EmbedText.cmake requires OMT_EMBED_OUTPUT and OMT_EMBED_INPUTS")
endif()

# CMake's regular expressions have no `{n}` repetition, so the wrap pattern is
# built by repeating one byte's worth of pattern.
set(byte_pattern "0x[0-9a-f][0-9a-f],")
set(row_pattern "")
foreach(column RANGE 1 12)
    string(APPEND row_pattern "${byte_pattern}")
endforeach()

set(arrays "")
set(table "")
set(index 0)
foreach(input IN LISTS OMT_EMBED_INPUTS)
    if(NOT EXISTS "${input}")
        message(FATAL_ERROR "EmbedText.cmake cannot read ${input}")
    endif()
    file(SIZE "${input}" size)
    if(size EQUAL 0)
        message(FATAL_ERROR "EmbedText.cmake refuses to embed the empty file ${input}")
    endif()
    get_filename_component(name "${input}" NAME)

    file(READ "${input}" hex HEX)
    string(REGEX REPLACE "([0-9a-f][0-9a-f])" "0x\\1," bytes "${hex}")
    # Wrap the array so the generated file stays within the line lengths every
    # supported compiler accepts and remains readable in a build log.
    string(REGEX REPLACE "(${row_pattern})" "\\1\n    " bytes "${bytes}")

    string(APPEND arrays "static const unsigned char document_${index}[] = {\n    ${bytes}\n};\n")
    string(APPEND table
        "    {\"${name}\", (const char *)document_${index}, sizeof(document_${index})},\n")
    math(EXPR index "${index} + 1")
endforeach()

configure_file("${CMAKE_CURRENT_LIST_DIR}/EmbedText.c.in" "${OMT_EMBED_OUTPUT}" @ONLY)
