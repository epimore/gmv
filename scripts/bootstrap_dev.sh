#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GMV_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$GMV_ROOT"

echo "[1/3] build FFmpeg"
bash ./stream/build_ffmpeg_min_bootstrap.sh

echo "[2/3] build PJSIP"
bash ./session/gb28181/build_pjsip_bootstrap.sh

echo "[3/3] cargo fetch"
cargo fetch

echo "Done."