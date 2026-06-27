#!/usr/bin/env bash
set -euo pipefail

PJSIP_VERSION="${PJSIP_VERSION:-2.17}"
JOBS="${JOBS:-$(nproc)}"
FORCE_REBUILD="${FORCE_REBUILD:-0}"
FORCE_REDOWNLOAD="${FORCE_REDOWNLOAD:-0}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GMV_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
THIRD_PARTY_DIR="$GMV_ROOT/third_party"

SRC_DIR="$THIRD_PARTY_DIR/pjproject-${PJSIP_VERSION}"
ARCHIVE_PATH="$THIRD_PARTY_DIR/pjproject-${PJSIP_VERSION}.tar.gz"
PREFIX="$SRC_DIR/dist"

SRC_URL="https://github.com/pjsip/pjproject/archive/refs/tags/${PJSIP_VERSION}.tar.gz"

echo "== PJSIP bootstrap build =="
echo "gmv root:   $GMV_ROOT"
echo "version:    $PJSIP_VERSION"
echo "source dir: $SRC_DIR"
echo "prefix:     $PREFIX"
echo "jobs:       $JOBS"
echo

mkdir -p "$THIRD_PARTY_DIR"

if [[ "$FORCE_REDOWNLOAD" == "1" ]]; then
  rm -rf "$SRC_DIR"
  rm -f "$ARCHIVE_PATH"
fi

if [[ "$FORCE_REBUILD" == "0" ]] \
  && [[ -f "$PREFIX/include/pjsip.h" ]] \
  && ls "$PREFIX/lib"/libpjsip*.a >/dev/null 2>&1; then
  echo "PJSIP already built: $PREFIX"
  exit 0
fi

if [[ ! -f "$SRC_DIR/configure" ]]; then
  echo "[1/8] download source"
  if [[ ! -f "$ARCHIVE_PATH" ]]; then
    curl -fL "$SRC_URL" -o "$ARCHIVE_PATH"
  else
    echo "archive already exists: $ARCHIVE_PATH"
  fi

  echo "[2/8] extract source"
  rm -rf "$SRC_DIR"
  mkdir -p "$SRC_DIR"
  tar -xf "$ARCHIVE_PATH" --strip-components=1 -C "$SRC_DIR"
else
  echo "[1/8] source already present, skip download"
fi

cd "$SRC_DIR"

echo "[3/8] write config_site.h"
mkdir -p pjlib/include/pj
cat > pjlib/include/pj/config_site.h <<'EOF'
#ifndef __PJ_CONFIG_SITE_H__
#define __PJ_CONFIG_SITE_H__

#define PJ_HAS_IPV6 1
#define PJ_HAS_SSL_SOCK 0

#define PJMEDIA_HAS_VIDEO 0
#define PJMEDIA_HAS_SRTP 0
#define PJMEDIA_HAS_WEBRTC_AEC 0
#define PJMEDIA_HAS_SPEEX_AEC 0

#define PJMEDIA_HAS_G711_CODEC 0
#define PJMEDIA_HAS_GSM_CODEC 0
#define PJMEDIA_HAS_SPEEX_CODEC 0
#define PJMEDIA_HAS_ILBC_CODEC 0
#define PJMEDIA_HAS_L16_CODEC 0
#define PJMEDIA_HAS_G722_CODEC 0
#define PJMEDIA_HAS_G7221_CODEC 0
#define PJMEDIA_HAS_OPENCORE_AMRNB_CODEC 0
#define PJMEDIA_HAS_OPENCORE_AMRWB_CODEC 0

#endif
EOF

echo "[4/8] distclean"
make distclean >/dev/null 2>&1 || true

echo "[5/8] purge old install"
rm -rf "$PREFIX"
mkdir -p "$PREFIX"

export CFLAGS="-O2 -fPIC"
export CXXFLAGS="-O2 -fPIC"

echo "[6/8] configure"
./configure \
  --prefix="$PREFIX" \
  --disable-shared \
  --enable-static \
  --disable-sound \
  --disable-video \
  --disable-opencore-amr \
  --disable-silk \
  --disable-speex-aec \
  --disable-gsm-codec \
  --disable-l16-codec \
  --disable-g722-codec \
  --disable-g7221-codec \
  --disable-ilbc-codec

echo "[7/8] build"
make dep
make -j"$JOBS"

echo "[8/8] install + verify"
make install

test -f "$PREFIX/include/pjsip.h"
test -f "$PREFIX/include/pjlib.h"
ls "$PREFIX/lib"/libpjsip*.a >/dev/null

echo
echo "Done: PJSIP static build complete"
echo "Install prefix: $PREFIX"