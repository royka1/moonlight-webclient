// Weak default implementations of the mlw_js_* extern symbols.
//
// These exist so that the WASM module links cleanly when run under tests
// or before the JS-side glue is wired up. In the real build, emscripten
// will resolve these symbols to the JS implementations provided via
// `--js-library moonlight_imports.js` (added in a future build.sh step).
//
// Once that JS library is in place, the linker will prefer the strong
// JS definitions and these weak stubs will be dropped.

#include "bindings.h"

#include <stdio.h>

__attribute__((weak)) void mlw_js_transport_open(int channel, const char* host, int port, int proto) {
    (void)channel; (void)host; (void)port; (void)proto;
}

__attribute__((weak)) int mlw_js_transport_send(int channel, const uint8_t* data, int len) {
    (void)channel; (void)data; (void)len;
    return -1;
}

__attribute__((weak)) void mlw_js_transport_close(int channel) { (void)channel; }

__attribute__((weak)) void mlw_js_video_submit(const uint8_t* data, int len, uint64_t pts_us, int flags) {
    (void)data; (void)len; (void)pts_us; (void)flags;
}

__attribute__((weak)) void mlw_js_audio_submit(const uint8_t* data, int len) {
    (void)data; (void)len;
}

__attribute__((weak)) void mlw_js_stage_starting(int stage) { (void)stage; }
__attribute__((weak)) void mlw_js_stage_failed(int stage, int err) { (void)stage; (void)err; }
__attribute__((weak)) void mlw_js_connection_started(void) {}
__attribute__((weak)) void mlw_js_connection_terminated(int err) { (void)err; }
__attribute__((weak)) void mlw_js_log(const char* msg) { fputs(msg, stderr); }
__attribute__((weak)) void mlw_js_rumble(uint16_t c, uint16_t l, uint16_t h) {
    (void)c; (void)l; (void)h;
}
__attribute__((weak)) void mlw_js_http_response(int id, int status, const uint8_t* body, int len) {
    (void)id; (void)status; (void)body; (void)len;
}
