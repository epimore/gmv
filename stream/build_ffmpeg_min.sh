#!/usr/bin/env bash
set -euo pipefail

### ===== configurable =====
PREFIX=/home/ubuntu20/code/c/ffmpeg/build64
JOBS=$(nproc)

### ===== sanity =====
if [ ! -f configure ]; then
  echo "[FATAL] must run in ffmpeg source root"
  exit 1
fi

echo "== FFmpeg safe rebuild =="
echo "prefix: $PREFIX"
echo "jobs:   $JOBS"
echo

### ===== hard clean =====
echo "[1/6] distclean"
make distclean >/dev/null 2>&1 || true

### ===== purge old install =====
echo "[2/6] purge old install"
rm -rf \
  "$PREFIX/lib/libav"*.a \
  "$PREFIX/include/libav"* \
  "$PREFIX/lib/pkgconfig/libav"*.pc

### ===== flags =====
export CFLAGS="-O2 -fPIC"
export CXXFLAGS="-O2 -fPIC"
export LDFLAGS="-static"

### ===== configure =====
echo "[3/6] configure"

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

### ===== build =====
echo "[4/6] build"
make -j"$JOBS"

### ===== install =====
echo "[5/6] install"
make install

### ===== verify =====
echo "[6/6] verify"

echo "→ static libs:"
ls -lh "$PREFIX/lib/libav"*.a

echo
echo "→ ensure no shared libs:"
if ls "$PREFIX/lib/libav"*.so >/dev/null 2>&1; then
  echo "[ERROR] shared libs found"
  exit 1
else
  echo "OK"
fi

echo
echo "→ check symbols (sanity):"
nm -g "$PREFIX/lib/libavformat.a" | head -n 5

echo
echo "✔ FFmpeg static safe rebuild DONE"
