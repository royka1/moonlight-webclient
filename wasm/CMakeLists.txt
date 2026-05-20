cmake_minimum_required(VERSION 3.20)
project(moonlight_wasm C)

if(NOT DEFINED COMMON_C_DIR)
    set(COMMON_C_DIR "${CMAKE_SOURCE_DIR}/../../moonlight-common-c")
endif()
if(NOT EXISTS "${COMMON_C_DIR}/src/Limelight.h")
    message(FATAL_ERROR "moonlight-common-c not found at ${COMMON_C_DIR}")
endif()

if(NOT DEFINED MBEDTLS_INSTALL_DIR)
    set(MBEDTLS_INSTALL_DIR "${CMAKE_SOURCE_DIR}/deps/mbedtls-install")
endif()
if(NOT EXISTS "${MBEDTLS_INSTALL_DIR}/lib/libmbedcrypto.a")
    message(FATAL_ERROR "mbedtls not found at ${MBEDTLS_INSTALL_DIR}. Run wasm/build.sh which will fetch and build it.")
endif()

set(CMAKE_C_STANDARD 11)

# Pick up every .c in moonlight-common-c/src; future upstream changes that
# add new files will be included automatically. This mirrors the upstream
# CMakeLists.txt's `file(GLOB SRC_LIST src/*.c)` behaviour.
#
# We exclude PlatformSockets.c - we provide a full replacement in
# src/platform_web.c that uses our channel multiplexer instead of libc
# sockets (browsers can't open raw UDP/TCP).
file(GLOB COMMON_C_SOURCES "${COMMON_C_DIR}/src/*.c")
list(FILTER COMMON_C_SOURCES EXCLUDE REGEX "PlatformSockets\\.c$")

# enet: keep the Unix sources, drop the Windows-only one (emscripten target
# is POSIX-shaped).
file(GLOB ENET_ALL "${COMMON_C_DIR}/enet/*.c")
list(FILTER ENET_ALL EXCLUDE REGEX ".*win32\\.c$")
set(ENET_SOURCES ${ENET_ALL})

# nanors: rs.c is #included from src/rswrapper.c (already part of
# COMMON_C_SOURCES), so we don't compile it standalone. The obl/simde
# header trees must be on the include path though.

set(BRIDGE_SOURCES
    "${CMAKE_SOURCE_DIR}/src/bindings.c"
    "${CMAKE_SOURCE_DIR}/src/platform_web.c"
)

add_executable(moonlight
    ${COMMON_C_SOURCES}
    ${ENET_SOURCES}
    ${BRIDGE_SOURCES}
)

target_include_directories(moonlight PRIVATE
    "${COMMON_C_DIR}/src"
    "${COMMON_C_DIR}/enet/include"
    "${COMMON_C_DIR}/nanors"
    "${COMMON_C_DIR}/nanors/deps"
    "${COMMON_C_DIR}/nanors/deps/obl"
    "${MBEDTLS_INSTALL_DIR}/include"
    "${CMAKE_SOURCE_DIR}/src"
)

target_compile_definitions(moonlight PRIVATE
    USE_MBEDTLS=1
    HAS_SOCKLEN_T=1
    LC_DEBUG=1
    # enet build flags.
    HAS_FCNTL=1
    HAS_POLL=1
    HAS_GETADDRINFO=1
    HAS_GETNAMEINFO=1
    HAS_GETHOSTBYNAME_R=1
    HAS_INET_PTON=1
    HAS_INET_NTOP=1
    HAS_SOCKLEN_T=1
    # Force ENet to use sendto/recvfrom instead of sendmsg/recvmsg. Our
    # sendto/recvfrom overrides are complete; sendmsg/recvmsg are stubs.
    NO_MSGAPI=1
)

set(EMSCRIPTEN_FLAGS
    "-pthread"
    "-O2"
    # -flto removed: wasm-ld --wrap doesn't intercept LTO-inlined call
    # sites, which silently bypasses every interception we set up in
    # platform_web.c (libc connect() ends up calling emscripten's
    # WebSocket-emulation createPeer() instead of our channel shim).
    "-Wno-unused-parameter"
    "-Wno-unused-but-set-variable"
)
target_compile_options(moonlight PRIVATE ${EMSCRIPTEN_FLAGS})

# Link mbedtls statically.
target_link_libraries(moonlight PRIVATE
    "${MBEDTLS_INSTALL_DIR}/lib/libmbedtls.a"
    "${MBEDTLS_INSTALL_DIR}/lib/libmbedx509.a"
    "${MBEDTLS_INSTALL_DIR}/lib/libmbedcrypto.a"
)

# Linker flags.
target_link_options(moonlight PRIVATE
    "-pthread"
    "-sUSE_PTHREADS=1"
    "-sPTHREAD_POOL_SIZE=8"
    "-sMODULARIZE=1"
    "-sEXPORT_ES6=1"
    "-sEXPORT_NAME=createMoonlightModule"
    "-sENVIRONMENT=web,worker"
    "-sALLOW_MEMORY_GROWTH=1"
    "-sINITIAL_MEMORY=64MB"
    "-sMAXIMUM_MEMORY=512MB"
    "-sSTACK_SIZE=2MB"
    "-sASYNCIFY=1"
    "-sASYNCIFY_STACK_SIZE=65536"
    "-sEXPORTED_RUNTIME_METHODS=ccall,cwrap,HEAPU8,HEAP32,addFunction,removeFunction,UTF8ToString,stringToUTF8,lengthBytesUTF8"
    "-sEXPORTED_FUNCTIONS=_malloc,_free,_mlw_init,_mlw_start,_mlw_start_async,_mlw_stop,_mlw_send_mouse_move,_mlw_send_mouse_position,_mlw_send_mouse_button,_mlw_send_keyboard,_mlw_send_scroll,_mlw_send_controller,_mlw_send_controller_arrival,_mlw_request_idr,_mlw_get_negotiated_format,_mlw_pair,_mlw_http_request,_mlw_inbound_packet"
    "-sALLOW_TABLE_GROWTH=1"
    "-sFETCH=1"
    "--no-entry"
    "--js-library=${CMAKE_SOURCE_DIR}/moonlight_imports.js"
    # NB: deliberately NOT linking -lwebsocket.js. moonlight-common-c never
    # reaches libc sockets in this build (PlatformSockets.c is replaced by
    # src/platform_web.c, which talks to the host proxy directly). If any
    # other code path does call libc connect(), we'd rather see a clean
    # link error than have emscripten silently open a raw ws:// connection
    # that mixed-content would block anyway.
)

set_target_properties(moonlight PROPERTIES
    SUFFIX ".js"
    OUTPUT_NAME "moonlight"
    LINK_DEPENDS "${CMAKE_SOURCE_DIR}/moonlight_imports.js"
)
