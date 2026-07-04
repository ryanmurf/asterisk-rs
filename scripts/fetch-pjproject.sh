#!/usr/bin/env bash
#
# fetch-pjproject.sh -- download the pjproject 2.16 source tree that the
# `pjsip-shim` crate compiles against when built with the `pjproject-cffi`
# feature.
#
# The tree is placed at crates/pjsip-shim/vendor/pjproject-2.16, which the
# crate's build.rs auto-detects. That directory is git-ignored, so the
# third-party (GPL-2.0) sources are never committed to this repo.
#
# Usage:
#   scripts/fetch-pjproject.sh
#   cargo build -p pjsip-shim --features pjproject-cffi --release
#
# Override the version or download URL via env vars if needed:
#   PJ_VERSION=2.16 PJ_URL=https://.../2.16.tar.gz scripts/fetch-pjproject.sh
#
set -euo pipefail

PJ_VERSION="${PJ_VERSION:-2.16}"
PJ_URL="${PJ_URL:-https://github.com/pjsip/pjproject/archive/refs/tags/${PJ_VERSION}.tar.gz}"

# Resolve repo paths relative to this script so it works from any CWD.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
VENDOR_DIR="${REPO_ROOT}/crates/pjsip-shim/vendor"
DEST_DIR="${VENDOR_DIR}/pjproject-${PJ_VERSION}"
TYPES_H="${DEST_DIR}/pjlib/include/pj/types.h"
CONFIG_SITE="${DEST_DIR}/pjlib/include/pj/config_site.h"

log() { printf '==> %s\n' "$*"; }

download() {
    local url="$1" out="$2"
    if command -v curl >/dev/null 2>&1; then
        curl -fSL --retry 3 -o "$out" "$url"
    elif command -v wget >/dev/null 2>&1; then
        wget -O "$out" "$url"
    else
        echo "error: neither curl nor wget is available" >&2
        exit 1
    fi
}

if [ -f "${TYPES_H}" ]; then
    log "pjproject ${PJ_VERSION} already present at ${DEST_DIR}"
else
    mkdir -p "${VENDOR_DIR}"
    tmp_tarball="${VENDOR_DIR}/pjproject-${PJ_VERSION}.tar.gz"
    log "downloading pjproject ${PJ_VERSION} from ${PJ_URL}"
    download "${PJ_URL}" "${tmp_tarball}"
    log "extracting into ${VENDOR_DIR}"
    tar -xzf "${tmp_tarball}" -C "${VENDOR_DIR}"
    rm -f "${tmp_tarball}"
    if [ ! -f "${TYPES_H}" ]; then
        echo "error: extraction did not yield ${TYPES_H}" >&2
        echo "       (unexpected tarball layout for ${PJ_URL})" >&2
        exit 1
    fi
fi

# pjproject requires a config_site.h (normally created by the user). An empty
# file selects all defaults, which is what the shim's build.rs expects.
if [ ! -f "${CONFIG_SITE}" ]; then
    log "creating default config_site.h"
    : > "${CONFIG_SITE}"
fi

# PJ_AUTOCONF=1 (set by build.rs) makes pjproject include the generated
# pj/compat/*_auto.h headers. Run pjproject's own configure to produce them
# if they are not already there. This is idempotent.
if [ ! -f "${DEST_DIR}/pjlib/include/pj/compat/os_auto.h" ]; then
    log "running pjproject ./configure to generate autoconf headers"
    ( cd "${DEST_DIR}" && ./configure >/dev/null )
fi

log "done. pjproject ${PJ_VERSION} ready at:"
log "  ${DEST_DIR}"
log "build the shim with:"
log "  cargo build -p pjsip-shim --features pjproject-cffi --release"
