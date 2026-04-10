#!/usr/bin/env bash
set -euo pipefail

# Minimal FFmpeg static build bootstrap for gmv developers.
# - Downloads FFmpeg source into gmv/third_party
# - Builds static libs with the same feature set as build_ffmpeg_min.sh
# - Installs into <source>/dist

FFMPEG_VERSION="${FFMPEG_VERSION:-6.1}"
JOBS="${JOBS:-$(nproc)}"
FORCE_REDOWNLOAD="${FORCE_REDOWNLOAD:-0}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GMV_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
THIRD_PARTY_DIR="$GMV_ROOT/third_party"
SRC_DIR="$THIRD_PARTY_DIR/ffmpeg-${FFMPEG_VERSION}"
ARCHIVE_PATH="$THIRD_PARTY_DIR/ffmpeg-${FFMPEG_VERSION}.tar.gz"
PREFIX="$SRC_DIR/dist"

# GitHub tag tarball, e.g. n6.1
SRC_URL="https://github.com/FFmpeg/FFmpeg/archive/refs/tags/n${FFMPEG_VERSION}.tar.gz"

echo "== FFmpeg bootstrap build =="
echo "gmv root:   $GMV_ROOT"
echo "version:    $FFMPEG_VERSION"
echo "source dir: $SRC_DIR"
echo "prefix:     $PREFIX"
echo "jobs:       $JOBS"
echo

mkdir -p "$THIRD_PARTY_DIR"

if [[ "$FORCE_REDOWNLOAD" == "1" ]]; then
  echo "[0/7] force cleanup existing source/archive"
  rm -rf "$SRC_DIR"
  rm -f "$ARCHIVE_PATH"
fi

if [[ ! -f "$SRC_DIR/configure" ]]; then
  echo "[1/7] download source"
  if [[ ! -f "$ARCHIVE_PATH" ]]; then
    curl -fL "$SRC_URL" -o "$ARCHIVE_PATH"
  else
    echo "archive already exists: $ARCHIVE_PATH"
  fi

  echo "[2/7] extract source"
  rm -rf "$SRC_DIR"
  mkdir -p "$SRC_DIR"
  tar -xf "$ARCHIVE_PATH" --strip-components=1 -C "$SRC_DIR"
else
  echo "[1/7] source already present, skip download"
fi

cd "$SRC_DIR"

if [[ ! -f configure ]]; then
  echo "[FATAL] configure not found in $SRC_DIR"
  exit 1
fi

echo "[3/7] distclean"
make distclean >/dev/null 2>&1 || true

echo "[4/7] purge old install"
rm -rf "$PREFIX"
mkdir -p "$PREFIX"

export CFLAGS="-O2 -fPIC"
export CXXFLAGS="-O2 -fPIC"
export LDFLAGS="-static"

echo "[5/7] configure"
./configure \
  --prefix="$PREFIX" \
  \
  --enable-static \
  --disable-shared \
  --enable-pic \
  \
  --enable-gpl \
  --enable-version3 \
  --disable-debug \
  --disable-doc \
  --disable-programs \
  --extra-cflags="-fPIC -I/usr/local/include" \
  --extra-ldflags="-L/usr/local/lib" \
  \
  --enable-avdevice \
  --enable-avcodec \
  --enable-avformat \
  --disable-postproc \
  --enable-swscale \
  --enable-swresample \
  \
  --disable-network \
  \
  --disable-everything \
  \
  --enable-protocol=file \
  \
  --enable-demuxer=mov \
  --enable-demuxer=flv \
  --enable-demuxer=mpegts \
  --enable-demuxer=mpegps \
  --enable-demuxer=h264 \
  --enable-demuxer=hevc \
  --enable-demuxer=aac \
  \
  --enable-muxer=mov \
  --enable-muxer=mp4 \
  --enable-muxer=flv \
  --enable-muxer=mpegts \
  --enable-muxer=hls \
  --enable-muxer=dash \
  \
  --enable-decoder=h264 \
  --enable-decoder=hevc \
  --enable-decoder=aac \
  --enable-decoder=pcm_alaw \
  --enable-decoder=pcm_mulaw \
  \
  --enable-encoder=aac \
  --enable-encoder=pcm_alaw \
  --enable-encoder=pcm_mulaw \
  \
  --enable-parser=h264 \
  --enable-parser=hevc \
  --enable-parser=aac \
  \
  --enable-bsf=h264_mp4toannexb \
  --enable-bsf=hevc_mp4toannexb \
  --enable-bsf=aac_adtstoasc \
  \
  --disable-filters \
  --disable-devices \
  --disable-hwaccels \
  --disable-indevs \
  --disable-outdevs \
  \
  --disable-iconv \
  --disable-zlib \
  --disable-lzma \
  --disable-bzlib \
  --disable-openssl \
  --disable-gnutls

echo "[6/7] build"
make -j"$JOBS"

echo "[7/7] install + verify"
make install

echo "-> static libs:"
ls -lh "$PREFIX/lib/libav"*.a

echo
echo "-> ensure no shared libs:"
if ls "$PREFIX/lib/libav"*.so >/dev/null 2>&1; then
  echo "[ERROR] shared libs found"
  exit 1
else
  echo "OK"
fi

echo
echo "-> check symbols (sanity):"
nm -g "$PREFIX/lib/libavformat.a" | head -n 5 || true

echo
echo "Done: FFmpeg static minimal build complete"
echo "Install prefix: $PREFIX"
