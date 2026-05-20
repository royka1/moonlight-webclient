#!/usr/bin/env bash
# Build moonlight-common-c (and its mbedtls dependency) to WebAssembly via
# Emscripten. Produces public/wasm/moonlight.{js,wasm,worker.js}.
#
# Prerequisites:
#   - emsdk activated in the current shell ('source emsdk_env.sh')
#   - cmake >= 3.20
#   - git
#   - The moonlight-common-c source tree at $COMMON_C_DIR (defaults to
#     ../../moonlight-common-c, where it lives in the parent repo).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUILD_DIR="${SCRIPT_DIR}/build"
DEPS_DIR="${SCRIPT_DIR}/deps"
OUT_DIR="${SCRIPT_DIR}/../public/wasm"

# Find the moonlight-common-c source tree. Honour an explicit COMMON_C_DIR
# override; otherwise look in the standard places (newest first):
#   1. vendor/moonlight-common-c next to the moonlight-webapp repo root
#      (the layout used after `git submodule add` for this repo)
#   2. wasm/moonlight-common-c (also a valid submodule location)
#   3. ../../moonlight-common-c (the original layout where the repos live
#      side-by-side in a parent folder)
if [[ -z "${COMMON_C_DIR:-}" ]]; then
  for candidate in \
    "${SCRIPT_DIR}/../vendor/moonlight-common-c" \
    "${SCRIPT_DIR}/moonlight-common-c" \
    "${SCRIPT_DIR}/../../moonlight-common-c"; do
    if [[ -d "${candidate}/src" ]]; then
      COMMON_C_DIR="${candidate}"
      break
    fi
  done
fi
: "${COMMON_C_DIR:=${SCRIPT_DIR}/../vendor/moonlight-common-c}"

: "${MBEDTLS_VERSION:=v3.6.2}"

if ! command -v emcmake >/dev/null 2>&1; then
  echo "error: emcmake not found. Activate emsdk first:" >&2
  echo "  source /path/to/emsdk/emsdk_env.sh" >&2
  exit 1
fi

if [[ ! -d "${COMMON_C_DIR}/src" ]]; then
  echo "error: moonlight-common-c not found at ${COMMON_C_DIR}" >&2
  echo "Set COMMON_C_DIR or run 'git submodule update --init --recursive' in the parent repo." >&2
  exit 1
fi

mkdir -p "${BUILD_DIR}" "${OUT_DIR}" "${DEPS_DIR}"

# ---------- mbedtls (built once, cached in deps/mbedtls-install) ----------

MBEDTLS_SRC="${DEPS_DIR}/mbedtls"
MBEDTLS_BUILD="${DEPS_DIR}/mbedtls-build"
MBEDTLS_INSTALL="${DEPS_DIR}/mbedtls-install"

if [[ ! -d "${MBEDTLS_SRC}" ]]; then
  echo "==> Cloning mbedtls ${MBEDTLS_VERSION}"
  git clone --depth 1 --branch "${MBEDTLS_VERSION}" \
    https://github.com/Mbed-TLS/mbedtls.git "${MBEDTLS_SRC}"
  git -C "${MBEDTLS_SRC}" submodule update --init --recursive --depth 1
fi

if [[ ! -f "${MBEDTLS_INSTALL}/lib/libmbedcrypto.a" ]]; then
  echo "==> Building mbedtls for wasm"
  rm -rf "${MBEDTLS_BUILD}"
  mkdir -p "${MBEDTLS_BUILD}"
  emcmake cmake -S "${MBEDTLS_SRC}" -B "${MBEDTLS_BUILD}" \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_INSTALL_PREFIX="${MBEDTLS_INSTALL}" \
    -DENABLE_TESTING=OFF \
    -DENABLE_PROGRAMS=OFF \
    -DUSE_SHARED_MBEDTLS_LIBRARY=OFF \
    -DMBEDTLS_FATAL_WARNINGS=OFF \
    -DCMAKE_C_FLAGS="-pthread -O3 -Wno-error"
  cmake --build "${MBEDTLS_BUILD}" --target install -j"$(nproc 2>/dev/null || echo 4)"
fi

# ---------- moonlight-common-c + bindings ----------

pushd "${BUILD_DIR}" >/dev/null

emcmake cmake "${SCRIPT_DIR}" \
  -DCMAKE_BUILD_TYPE=Release \
  -DCOMMON_C_DIR="${COMMON_C_DIR}" \
  -DMBEDTLS_INSTALL_DIR="${MBEDTLS_INSTALL}"

emmake make -j"$(nproc 2>/dev/null || echo 4)"

cp moonlight.js moonlight.wasm "${OUT_DIR}/"
if [[ -f moonlight.worker.js ]]; then
  cp moonlight.worker.js "${OUT_DIR}/"
fi

popd >/dev/null

echo
echo "Built artefacts:"
ls -lh "${OUT_DIR}"
