#!/usr/bin/env bash
set -euo pipefail

# Usage:
#   source stream/env_ffmpeg.sh
# Optional:
#   source stream/env_ffmpeg.sh /abs/path/to/ffmpeg-dist

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GMV_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
THIRD_PARTY_DIR="$GMV_ROOT/third_party"

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  echo "[ERROR] Please source this script, do not execute it:"
  echo "        source stream/env_ffmpeg.sh"
  exit 1
fi

# Resolve FFMPEG_ROOT
if [[ $# -ge 1 && -n "${1:-}" ]]; then
  FFMPEG_ROOT_INPUT="$1"
else
  # Pick latest ffmpeg-*/dist by version sort
  FFMPEG_ROOT_INPUT="$(find "$THIRD_PARTY_DIR" -maxdepth 2 -type d -name dist -path '*/ffmpeg-*/dist' 2>/dev/null | sort -V | tail -n 1 || true)"
fi

if [[ -z "$FFMPEG_ROOT_INPUT" ]]; then
  echo "[ERROR] No ffmpeg dist directory found under: $THIRD_PARTY_DIR"
  echo "Run: ./stream/build_ffmpeg_min_bootstrap.sh"
  return 1
fi

if [[ ! -d "$FFMPEG_ROOT_INPUT" ]]; then
  echo "[ERROR] FFMPEG_ROOT does not exist: $FFMPEG_ROOT_INPUT"
  return 1
fi

if [[ ! -d "$FFMPEG_ROOT_INPUT/lib/pkgconfig" ]]; then
  echo "[ERROR] Missing pkgconfig dir: $FFMPEG_ROOT_INPUT/lib/pkgconfig"
  return 1
fi

export FFMPEG_ROOT="$FFMPEG_ROOT_INPUT"

# Prepend helper to avoid duplicate path growth.
prepend_path() {
  local var_name="$1"
  local path_value="$2"
  local current="${!var_name-}"
  if [[ -z "$current" ]]; then
    export "$var_name=$path_value"
  elif [[ ":$current:" != *":$path_value:"* ]]; then
    export "$var_name=$path_value:$current"
  fi
}

prepend_path PKG_CONFIG_PATH "$FFMPEG_ROOT/lib/pkgconfig"
prepend_path LD_LIBRARY_PATH "$FFMPEG_ROOT/lib"
prepend_path LIBRARY_PATH "$FFMPEG_ROOT/lib"
prepend_path C_INCLUDE_PATH "$FFMPEG_ROOT/include"
prepend_path CPLUS_INCLUDE_PATH "$FFMPEG_ROOT/include"

echo "[OK] FFmpeg environment loaded"
echo "FFMPEG_ROOT=$FFMPEG_ROOT"
echo "PKG_CONFIG_PATH=$PKG_CONFIG_PATH"
