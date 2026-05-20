// JS <-> C glue for moonlight-common-c on the web. Implements the
// `mlw_*` entry points declared in bindings.h and registers the
// renderer / connection-listener callback structs with libmoonlight.

#include "bindings.h"

#include <Limelight.h>

#include <pthread.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

// Callbacks back into JS. The JS library (moonlight_imports.js) handles
// the cross-worker routing: on the main wasm worker it calls
// Module.{videoSink,audioSink,events} directly; on a pthread worker it
// postMessages the call to the parent (main wasm worker).
#define MLW_SUBMIT_VIDEO(data, len, ptsUs, flags) \
    mlw_js_video_submit((const uint8_t*)(data), (int)(len), \
                        (uint32_t)((uint64_t)(ptsUs) & 0xFFFFFFFF), \
                        (uint32_t)((uint64_t)(ptsUs) >> 32), \
                        (int)(flags))
#define MLW_SUBMIT_AUDIO(data, len) \
    mlw_js_audio_submit((const uint8_t*)(data), (int)(len))
#define MLW_EVENT_STAGE_STARTING(stage)    mlw_js_stage_starting((int)(stage))
#define MLW_EVENT_STAGE_FAILED(stage, err) mlw_js_stage_failed((int)(stage), (int)(err))
#define MLW_EVENT_CONNECTED()              mlw_js_connection_started()
#define MLW_EVENT_TERMINATED(err)          mlw_js_connection_terminated((int)(err))
#define MLW_EVENT_LOG(msg)                 mlw_js_log((const char*)(msg))
#define MLW_EVENT_RUMBLE(c, lo, hi)        mlw_js_rumble((uint16_t)(c), (uint16_t)(lo), (uint16_t)(hi))
#define MLW_EVENT_VIDEO_FORMAT(fmt)        mlw_js_video_format((int)(fmt))

// ---------- Decoder/Renderer callbacks ----------

static int s_negotiated_format = 0;

// Tracks whether we've seen the first IDR for the current session. Until
// then, every P-frame returns DR_NEED_IDR so moonlight-common-c asks
// Sunshine to send a fresh IDR. Without this, if we miss the initial IDR
// (RTP loss, slow proxy ramp-up, late JS-side decoder init), every
// subsequent P-frame goes nowhere and the screen stays black indefinitely.
static int s_first_idr_received = 0;

int mlw_get_negotiated_format(void) {
    return s_negotiated_format;
}

static int vidDecSetup(int videoFormat, int width, int height,
                       int redrawRate, void* ctx, int drFlags) {
    (void)width; (void)height; (void)redrawRate; (void)ctx; (void)drFlags;
    s_negotiated_format = videoFormat;
    s_first_idr_received = 0;  // fresh session, force IDR request
    char msg[128];
    snprintf(msg, sizeof(msg), "[bindings] vidDecSetup: format=0x%x (%s)",
             videoFormat,
             (videoFormat & 0x000F) ? "h264" :
             (videoFormat & 0x0F00) ? "hevc" :
             (videoFormat & 0xF000) ? "av1" : "unknown");
    MLW_EVENT_LOG(msg);
    MLW_EVENT_VIDEO_FORMAT(videoFormat);
    return 0;
}

static void vidDecStart(void)   {}
static void vidDecStop(void)    {}
static void vidDecCleanup(void) {}

static int vidDecSubmit(PDECODE_UNIT du) {
    static int s_log_count = 0;
    if (s_log_count < 20 || du->frameType == FRAME_TYPE_IDR) {
        fprintf(stderr, "[bindings] frame type=%d (%s) length=%d first_idr=%d\n",
                du->frameType,
                du->frameType == FRAME_TYPE_IDR ? "IDR" : "P/B",
                du->fullLength, s_first_idr_received);
        s_log_count++;
    }

    if (du->frameType != FRAME_TYPE_IDR && !s_first_idr_received) {
        // We can't decode P/B frames without a reference. Tell moonlight
        // to request a keyframe from Sunshine.
        return DR_NEED_IDR;
    }

    // moonlight-common-c's PLENTRY data already contains Annex-B start
    // codes - the depacketizer parses them from the wire and copies them
    // into each entry. moonlight-android just concatenates the entries
    // and hands the flat buffer to MediaCodec; we do the same for
    // WebCodecs. DO NOT prepend extra start codes - WebCodecs (and
    // every H.264/HEVC/AV1 decoder for that matter) silently rejects
    // bitstreams with `00 00 00 01 00 00 00 01` sequences, which is why
    // every codec produces a black screen otherwise.
    uint8_t* buf = (uint8_t*)malloc(du->fullLength);
    if (!buf) return DR_NEED_IDR;

    int offset = 0;
    PLENTRY entry = du->bufferList;
    while (entry) {
        memcpy(buf + offset, entry->data, entry->length);
        offset += entry->length;
        entry = entry->next;
    }

    int flags = 0;
    if (du->frameType == FRAME_TYPE_IDR) {
        flags |= 1;
        s_first_idr_received = 1;
    }
    MLW_SUBMIT_VIDEO(buf, offset, du->presentationTimeUs, flags);
    return DR_OK;
}

static DECODER_RENDERER_CALLBACKS s_decoder = {
    .setup = vidDecSetup,
    .start = vidDecStart,
    .stop = vidDecStop,
    .cleanup = vidDecCleanup,
    .submitDecodeUnit = vidDecSubmit,
    .capabilities = CAPABILITY_REFERENCE_FRAME_INVALIDATION_HEVC |
                    CAPABILITY_REFERENCE_FRAME_INVALIDATION_AV1 |
                    CAPABILITY_DIRECT_SUBMIT,
};

// ---------- Audio callbacks ----------

static int audInit(int audioConfig, POPUS_MULTISTREAM_CONFIGURATION cfg,
                   void* ctx, int flags) {
    (void)audioConfig; (void)ctx; (void)flags;
    // Forward the Opus config to JS so the AudioWorklet can initialise its
    // own decoder. We pack into a small header buffer.
    uint8_t hdr[64];
    int o = 0;
    hdr[o++] = (uint8_t)cfg->sampleRate; hdr[o++] = (uint8_t)(cfg->sampleRate >> 8);
    hdr[o++] = (uint8_t)(cfg->sampleRate >> 16); hdr[o++] = (uint8_t)(cfg->sampleRate >> 24);
    hdr[o++] = cfg->channelCount;
    hdr[o++] = cfg->streams;
    hdr[o++] = cfg->coupledStreams;
    hdr[o++] = cfg->samplesPerFrame & 0xFF; hdr[o++] = (cfg->samplesPerFrame >> 8) & 0xFF;
    memcpy(hdr + o, cfg->mapping, 8);
    o += 8;
    // Signal "init packet" by sending a length of -1 ... we use a separate
    // mlw_js call instead, but for the scaffold we just punt the bytes.
    MLW_SUBMIT_AUDIO(hdr, o);
    return 0;
}

static void audStart(void)   {}
static void audStop(void)    {}
static void audCleanup(void) {}

static void audDecode(char* data, int len) {
    MLW_SUBMIT_AUDIO((const uint8_t*)data, len);
}

static AUDIO_RENDERER_CALLBACKS s_audio = {
    .init = audInit,
    .start = audStart,
    .stop = audStop,
    .cleanup = audCleanup,
    .decodeAndPlaySample = audDecode,
    .capabilities = CAPABILITY_DIRECT_SUBMIT,
};

// ---------- Connection listener callbacks ----------

static void clStageStarting(int s)             { MLW_EVENT_STAGE_STARTING(s); }
static void clStageFailed(int s, int e)        { MLW_EVENT_STAGE_FAILED(s, e); }
static void clConnectionStarted(void)          { MLW_EVENT_CONNECTED(); }
static void clConnectionTerminated(int e)      { MLW_EVENT_TERMINATED(e); }
static void clLogMessage(const char* fmt, ...) {
    char buf[1024];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof(buf), fmt, ap);
    va_end(ap);
    MLW_EVENT_LOG(buf);
}
static void clRumble(uint16_t c, uint16_t lo, uint16_t hi) { MLW_EVENT_RUMBLE(c, lo, hi); }

static CONNECTION_LISTENER_CALLBACKS s_listener = {
    .stageStarting = clStageStarting,
    .stageFailed = clStageFailed,
    .connectionStarted = clConnectionStarted,
    .connectionTerminated = clConnectionTerminated,
    .logMessage = clLogMessage,
    .rumble = clRumble,
};

// ---------- Entry points ----------

int mlw_init(void) {
    // No-op for now; reserved for future state initialisation.
    return 0;
}

int mlw_start(const char* address,
              const char* app_version,
              const char* gfe_version,
              const char* rtsp_url,
              int width, int height, int fps,
              int bitrate_kbps, int packet_size,
              int video_format,
              int audio_config,
              int encryption_flags,
              const uint8_t* ri_key, int ri_key_len,
              int ri_key_id) {
    SERVER_INFORMATION server = {0};
    STREAM_CONFIGURATION config;

    LiInitializeServerInformation(&server);
    server.address = address;
    server.serverInfoAppVersion = app_version;
    server.serverInfoGfeVersion = gfe_version;
    server.rtspSessionUrl = rtsp_url;
    // SCM values come from Limelight.h — set all codecs Sunshine may support.
    // TODO: fetch from host /serverinfo and pass via mlw_start_async param.
    server.serverCodecModeSupport =
        SCM_H264 | SCM_HEVC | SCM_HEVC_MAIN10 |
        SCM_AV1_MAIN8 | SCM_AV1_MAIN10;

    LiInitializeStreamConfiguration(&config);
    config.width = width;
    config.height = height;
    config.fps = fps;
    config.bitrate = bitrate_kbps;
    config.packetSize = packet_size;
    config.streamingRemotely = STREAM_CFG_AUTO;
    config.audioConfiguration = audio_config;
    config.supportedVideoFormats = video_format;
    config.encryptionFlags = encryption_flags;

    if (ri_key_len == 16) {
        memcpy(config.remoteInputAesKey, ri_key, 16);
    }
    // moonlight-common-c expects the IV as the first 4 bytes of a 16-byte
    // buffer, big-endian.
    uint32_t iv_be = __builtin_bswap32((uint32_t)ri_key_id);
    memcpy(config.remoteInputAesIv, &iv_be, sizeof(iv_be));

    return LiStartConnection(&server, &config,
                             &s_listener, &s_decoder, &s_audio,
                             NULL, 0,
                             NULL, 0);
}

void mlw_stop(void)                    { LiStopConnection(); }
int  mlw_send_mouse_move(int dx,int dy){ return LiSendMouseMoveEvent((short)dx, (short)dy); }
int  mlw_send_mouse_position(int x,int y,int rw,int rh) {
    return LiSendMousePositionEvent((short)x,(short)y,(short)rw,(short)rh);
}
int  mlw_send_mouse_button(int a,int b){ return LiSendMouseButtonEvent((char)a,b); }
int  mlw_send_keyboard(int k,int a,int m){ return LiSendKeyboardEvent((short)k,(char)a,(char)m); }
int  mlw_send_scroll(int amt)          { return LiSendHighResScrollEvent((short)amt); }

int mlw_send_controller(int idx, int btns, int lt, int rt,
                        int lsx, int lsy, int rsx, int rsy) {
    return LiSendMultiControllerEvent((short)idx, 0xF,
                                      (short)btns,
                                      (unsigned char)lt, (unsigned char)rt,
                                      (short)lsx, (short)lsy,
                                      (short)rsx, (short)rsy);
}

int mlw_send_controller_arrival(int idx, int type, int supported, int caps) {
    return LiSendControllerArrivalEvent((uint8_t)idx, 0xF, (uint8_t)type,
                                        (uint32_t)supported, (uint16_t)caps);
}

void mlw_request_idr(void)             { LiRequestIdrFrame(); }

// Pairing / HTTP are intentionally stubbed in C - the JS side drives them
// directly against the host (with WebCrypto-backed TLS via fetch). See
// src/client/pairing.ts. The C-side function is here for future symmetry.
int mlw_pair(const char* address, const char* pin, int req_id) {
    (void)address; (void)pin; (void)req_id;
    return -1;
}

int mlw_http_request(const char* url, int req_id) {
    (void)url; (void)req_id;
    return -1;
}

// ---------- Async start ----------
//
// mlw_start() blocks the caller until LiStartConnection has finished the
// RTSP handshake (success or timeout). On the web, the calling thread is
// the worker that owns the WebSocket; while it's blocked, ws.onmessage
// can't fire, so the host's RTSP response never reaches the C side and
// we always time out. mlw_start_async() pthread_creates a thread to run
// the blocking call, freeing the worker's JS event loop.

typedef struct {
    char*    address;
    char*    app_version;
    char*    gfe_version;
    char*    rtsp_url;
    int      width;
    int      height;
    int      fps;
    int      bitrate_kbps;
    int      packet_size;
    int      video_format;
    int      audio_config;
    int      encryption_flags;
    uint8_t  ri_key[16];
    int      ri_key_id;
} mlw_start_args_t;

static void* mlw_start_thread_fn(void* arg) {
    mlw_start_args_t* a = (mlw_start_args_t*)arg;
    int err = mlw_start(a->address, a->app_version, a->gfe_version, a->rtsp_url,
                        a->width, a->height, a->fps, a->bitrate_kbps, a->packet_size,
                        a->video_format, a->audio_config, a->encryption_flags,
                        a->ri_key, 16, a->ri_key_id);
    if (err != 0) {
        // LiStartConnection returned an error before the connection-listener
        // callbacks could fire - surface it via onTerminated so the PWA gets
        // a signal that the session failed.
        MLW_EVENT_TERMINATED(err);
    }
    free(a->address);
    free(a->app_version);
    free(a->gfe_version);
    free(a->rtsp_url);
    free(a);
    return NULL;
}

int mlw_start_async(const char* address, const char* app_version,
                    const char* gfe_version, const char* rtsp_url,
                    int width, int height, int fps,
                    int bitrate_kbps, int packet_size,
                    int video_format, int audio_config, int encryption_flags,
                    const uint8_t* ri_key, int ri_key_len, int ri_key_id) {
    mlw_start_args_t* a = (mlw_start_args_t*)calloc(1, sizeof(*a));
    if (!a) return -1;
    a->address = strdup(address ? address : "");
    a->app_version = strdup(app_version ? app_version : "");
    a->gfe_version = strdup(gfe_version ? gfe_version : "");
    a->rtsp_url = strdup(rtsp_url ? rtsp_url : "");
    a->width = width;
    a->height = height;
    a->fps = fps;
    a->bitrate_kbps = bitrate_kbps;
    a->packet_size = packet_size;
    a->video_format = video_format;
    a->audio_config = audio_config;
    a->encryption_flags = encryption_flags;
    if (ri_key && ri_key_len == 16) memcpy(a->ri_key, ri_key, 16);
    a->ri_key_id = ri_key_id;

    pthread_t t;
    int rc = pthread_create(&t, NULL, mlw_start_thread_fn, a);
    if (rc != 0) {
        free(a->address); free(a->app_version);
        free(a->gfe_version); free(a->rtsp_url);
        free(a);
        return -1;
    }
    pthread_detach(t);
    return 0;
}
