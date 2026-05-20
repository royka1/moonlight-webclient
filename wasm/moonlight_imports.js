// Emscripten --js-library that links the mlw_js_* externs declared in
// bindings.h to JS implementations on the Module instance.
//
// CROSS-THREAD DISPATCH (this is load-bearing - read before changing):
//
// moonlight-common-c calls these functions from pthread workers (RTSP,
// video decoder, audio decoder, etc.). Each pthread worker has its own
// JS `Module` object - the Module.transport/videoSink/events we set on
// the main wasm worker are NOT visible there.
//
// `self.postMessage(...)` from inside a pthread worker doesn't help
// either: those messages arrive on the *specific Worker handle* the main
// wasm worker holds for that pthread, NOT on the main wasm worker's
// `self.onmessage` / `addEventListener('message')`. We can't intercept
// those without rewriting emscripten's pthread runtime.
//
// The right mechanism is emscripten's own proxy queue, opted into via
// the `__proxy` annotation:
//   - 'sync'  : pthread blocks until main runs the body and returns
//   - 'async' : fire-and-forget, main runs the body when its event loop
//               next gets a chance
// The accompanying `__sig` string declares the wasm signature so
// emscripten can marshal args correctly (i = i32, v = void, first char
// is the return type).

mergeInto(LibraryManager.library, {

  mlw_js_transport_open__proxy: 'sync',
  mlw_js_transport_open__sig: 'viiii',
  mlw_js_transport_open: function (channel, hostPtr, port, proto) {
    var host = UTF8ToString(hostPtr);
    if (Module.transport) Module.transport.open(channel, host, port, proto);
  },

  mlw_js_transport_send__proxy: 'sync',
  mlw_js_transport_send__sig: 'iiii',
  mlw_js_transport_send: function (channel, dataPtr, len) {
    if (!Module.transport) return -1;
    var buf = HEAPU8.slice(dataPtr, dataPtr + len);
    return Module.transport.send(channel, buf);
  },

  mlw_js_transport_close__proxy: 'sync',
  mlw_js_transport_close__sig: 'vi',
  mlw_js_transport_close: function (channel) {
    if (Module.transport) Module.transport.close(channel);
  },

  // Video frames are high-throughput; async proxy lets the producing
  // pthread continue immediately while the main worker processes.
  // IMPORTANT: vidDecSubmit malloc()s a fresh buffer for each frame.
  // We must _free(dataPtr) after copying so the buffer doesn't leak.
  mlw_js_video_submit__proxy: 'async',
  mlw_js_video_submit__sig: 'viiiii',
  mlw_js_video_submit: function (dataPtr, len, ptsUsLo, ptsUsHi, flags) {
    var buf = HEAPU8.slice(dataPtr, dataPtr + len);
    _free(dataPtr);
    // ptsUsLo/ptsUsHi may arrive as BigInt via the emscripten proxy queue;
    // coerce to Number before arithmetic (PTS fits in 53-bit safe integer
    // for ~285 years of µs timestamps).
    var pts = Number(ptsUsLo) + Number(ptsUsHi) * 0x100000000;

    // Diagnostic: log first frame header to verify Annex-B format.
    // Expected: [00 00 00 01 XX ...] where XX & 0x1f is the NAL type.
    if (!Module.__videoSubmitLogged && buf.length >= 5) {
      Module.__videoSubmitLogged = true;
      var nalType = buf[4] & 0x1f;
      var hex = [];
      for (var i = 0; i < Math.min(8, buf.length); i++) {
        hex.push(('0' + buf[i].toString(16)).slice(-2));
      }
      console.info('[video-imports] first frame: len=' + len +
        ' header=' + hex.join(' ') +
        ' nal_type=' + nalType + ' isKey=' + (flags & 1));
    }

    if (Module.videoSink) Module.videoSink(buf, pts, flags);
  },

  // Sync proxy: the audio buffer is owned by moonlight-common-c and reused
  // after this call returns. Sync ensures we copy before the next decode.
  mlw_js_audio_submit__proxy: 'sync',
  mlw_js_audio_submit__sig: 'vii',
  mlw_js_audio_submit: function (dataPtr, len) {
    var buf = HEAPU8.slice(dataPtr, dataPtr + len);
    if (Module.audioSink) Module.audioSink(buf);
  },

  mlw_js_stage_starting__proxy: 'async',
  mlw_js_stage_starting__sig: 'vi',
  mlw_js_stage_starting: function (stage) {
    if (Module.events && Module.events.onStage) Module.events.onStage(stage, 'starting');
  },

  mlw_js_stage_failed__proxy: 'async',
  mlw_js_stage_failed__sig: 'vii',
  mlw_js_stage_failed: function (stage, err) {
    if (Module.events && Module.events.onStage) Module.events.onStage(stage, 'failed', err);
  },

  mlw_js_connection_started__proxy: 'async',
  mlw_js_connection_started__sig: 'v',
  mlw_js_connection_started: function () {
    if (Module.events && Module.events.onConnected) Module.events.onConnected();
  },

  mlw_js_connection_terminated__proxy: 'async',
  mlw_js_connection_terminated__sig: 'vi',
  mlw_js_connection_terminated: function (err) {
    if (Module.events && Module.events.onTerminated) Module.events.onTerminated(err);
  },

  mlw_js_log__proxy: 'async',
  mlw_js_log__sig: 'vi',
  mlw_js_log: function (msgPtr) {
    var msg = UTF8ToString(msgPtr);
    if (Module.events && Module.events.onLog) Module.events.onLog(msg);
  },

  mlw_js_rumble__proxy: 'async',
  mlw_js_rumble__sig: 'viii',
  mlw_js_rumble: function (c, lo, hi) {
    if (Module.events && Module.events.onRumble) Module.events.onRumble(c, lo, hi);
  },

  // Sync proxy: the decoder pthread must NOT proceed until the JS side
  // has reconfigured the VideoDecoder for the negotiated codec. If we let
  // the pthread continue with async, AV1 frames can arrive before the
  // H.264→AV1 reconfig completes, producing a black screen.
  mlw_js_video_format__proxy: 'sync',
  mlw_js_video_format__sig: 'vi',
  mlw_js_video_format: function (format) {
    console.info('[imports] mlw_js_video_format: 0x' + format.toString(16) +
      ' env=' + ENVIRONMENT_IS_PTHREAD + ' events=' + !!Module.events);
    if (Module.events && Module.events.onVideoFormat) Module.events.onVideoFormat(format);
  },

  mlw_js_http_response__proxy: 'async',
  mlw_js_http_response__sig: 'viiii',
  mlw_js_http_response: function (id, status, dataPtr, len) {
    var buf = HEAPU8.slice(dataPtr, dataPtr + len);
    if (Module.events && Module.events.onHttp) Module.events.onHttp(id, status, buf);
  },
});
