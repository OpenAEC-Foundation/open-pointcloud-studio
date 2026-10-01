#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_dir"

case "${1:-run}" in
  fetch)
    mkdir -p "$project_dir/classic/dist"
    curl --fail --location --retry 3 \
      --output "$project_dir/classic/dist/Open.Pointcloud.Studio_0.3.0_amd64.deb" \
      "https://github.com/OpenAEC-Foundation/open-pointcloud-studio/releases/download/v0.3.0/Open.Pointcloud.Studio_0.3.0_amd64.deb"
    dpkg-deb -x "$project_dir/classic/dist/Open.Pointcloud.Studio_0.3.0_amd64.deb" \
      "$project_dir/classic/dist/extracted"
    ;;
  run)
    classic_binary="$project_dir/classic/dist/extracted/usr/bin/open-pointcloud-studio"
    if [[ ! -x "$classic_binary" ]]; then
      echo "Classic desktop binary is missing. Run './scripts/classic.sh fetch' or build from source with GTK/WebKit development packages installed." >&2
      exit 1
    fi
    exec "$classic_binary"
    ;;
  dev)
    command -v pnpm >/dev/null 2>&1 || { echo "pnpm is required for a source build" >&2; exit 1; }
    pnpm exec tauri dev --config src-tauri/tauri.classic.conf.json
    ;;
  build)
    command -v pnpm >/dev/null 2>&1 || { echo "pnpm is required for a source build" >&2; exit 1; }
    pnpm exec tauri build --config src-tauri/tauri.classic.conf.json
    ;;
  frontend)
    command -v pnpm >/dev/null 2>&1 || { echo "pnpm is required for a source build" >&2; exit 1; }
    pnpm build
    ;;
  *)
    echo "Usage: scripts/classic.sh [fetch|run|dev|build|frontend]" >&2
    exit 2
    ;;
esac
