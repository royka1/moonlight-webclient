#ifndef MOONLIGHT_WEB_BINDINGS_H
#define MOONLIGHT_WEB_BINDINGS_H

#include <stdint.h>

// JS imports - implemented in src/wasm/glue.ts and surfaced to wasm via
// emscripten's --js-library=moonlight_imports.js (added by build.sh once
// the JS-side glue is finalised). Until then, transport_bridge.c declares
// these as extern and they are linked at module instantiation.
//
// Naming convention: anything called from C into JS is prefixed `mlw_js_`.
// Anything called from JS into C is prefixed `mlw_`.

#ifdef __cplusplus
extern "C" {
#endif

// ---------- JS-side hooks (provided by glue) ----------

// Transport (host-proxy bridge).
extern void mlw_js_transport_open(int channel, const char* host, int port, int proto);
extern int  mlw_js_transport_send(int channel, const uint8_t* data, int len);
extern void mlw_js_transport_close(int channel);

// Renderer callbacks - emit decoded NALUs / Opus frames back to JS.
extern void mlw_js_video_submit(const uint8_t* data, int len, uint32_t pts_us_lo, uint32_t pts_us_hi, int flags);
extern void mlw_js_audio_submit(const uint8_t* data, int len);

// Stream lifecycle.
extern void mlw_js_stage_starting(int stage);
extern void mlw_js_stage_failed(int stage, int err);
extern void mlw_js_connection_started(void);
extern void mlw_js_connection_terminated(int err);
extern void mlw_js_log(const char* msg);
extern void mlw_js_rumble(uint16_t controller, uint16_t low, uint16_t high);
extern void mlw_js_video_format(int format);

// HTTP (handled by JS using fetch + WebCrypto for client cert auth).
extern void mlw_js_http_response(int req_id, int status, const uint8_t* body, int len);

// ---------- C-side entry points (called from JS) ----------

int  mlw_init(void);

// Begin a streaming session. Returns 0 on success.
// All string args are UTF-8.
//
// NB: mlw_start BLOCKS its caller until LiStartConnection finishes the
// RTSP handshake (success or timeout). On the web, calling this from the
// main wasm worker freezes that worker's JS event loop for ~15s on
// failure - which means ws.onmessage doesn't fire and we never see the
// host's response. Use mlw_start_async() unless you have your own thread.
int  mlw_start(const char* address,
               const char* app_version,
               const char* gfe_version,
               const char* rtsp_url,
               int width, int height, int fps,
               int bitrate_kbps, int packet_size,
               int video_format,
               int audio_config,
               int encryption_flags,
               const uint8_t* ri_key, int ri_key_len,
               int ri_key_id);

// Spawn a pthread that calls mlw_start with the given args. Returns 0 if
// the thread was successfully started. Connection success / failure is
// signalled via the existing connection-listener callbacks (onConnected /
// onTerminated). The caller can return immediately and keep its JS event
// loop responsive - critical for the worker that owns the WebSocket.
int  mlw_start_async(const char* address,
                     const char* app_version,
                     const char* gfe_version,
                     const char* rtsp_url,
                     int width, int height, int fps,
                     int bitrate_kbps, int packet_size,
                     int video_format,
                     int audio_config,
                     int encryption_flags,
                     const uint8_t* ri_key, int ri_key_len,
                     int ri_key_id);

void mlw_stop(void);

// Input.
int  mlw_send_mouse_move(int dx, int dy);
int  mlw_send_mouse_position(int x, int y, int ref_w, int ref_h);
int  mlw_send_mouse_button(int action, int button);
int  mlw_send_keyboard(int key_code, int action, int modifiers);
int  mlw_send_scroll(int amount);
int  mlw_send_controller(int controller_idx, int button_flags,
                         int left_trigger, int right_trigger,
                         int lsx, int lsy, int rsx, int rsy);
int  mlw_send_controller_arrival(int controller_idx, int type,
                                 int supported_buttons, int capabilities);
void mlw_request_idr(void);
int  mlw_get_negotiated_format(void);  // video format codec mask from RTSP negotiation

// Pairing and HTTP are driven from C (because the protocol is a
// pile of XML with HMAC challenge/response), but the underlying
// transport (TLS / cert auth / GET) is done in JS to leverage
// fetch + WebCrypto. mlw_pair launches the multi-step handshake.
int  mlw_pair(const char* address, const char* pin, int req_id);
int  mlw_http_request(const char* url, int req_id);

// Called from JS when a UDP packet arrives on `channel` (matching the
// channel that platform_web allocated for that virtual socket).
void mlw_inbound_packet(int channel, const uint8_t* data, int len);

#ifdef __cplusplus
}
#endif

#endif // MOONLIGHT_WEB_BINDINGS_H
