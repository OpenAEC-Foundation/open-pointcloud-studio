#!/usr/bin/env bash
# Run the extracted release under a headless Wayland compositor, without FUSE
# or a GPU. The Python probe starts only the product and its MCP transport.
set -euo pipefail
usage() { echo 'usage: managed-wayland-smoke.sh APP_RUN OUTPUT_DIRECTORY [--paused-access]' >&2; exit 2; }
[[ $# -ge 2 && $# -le 3 ]] || usage
probe=()
case ${3:-} in
    '') ;;
    --paused-access) probe=(--paused-access) ;;
    *) usage ;;
esac
binary=$(realpath "$1")
mkdir -p "$2"
output=$(realpath "$2")
scripts=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
work=$(mktemp -d)
compositor=
cleanup() {
    if [[ -n "$compositor" ]]; then
        kill "$compositor" 2>/dev/null || true
        wait "$compositor" 2>/dev/null || true
    fi
    rm -rf "$work"
}
trap cleanup EXIT
export XDG_RUNTIME_DIR="$work/runtime"
mkdir -m 700 "$XDG_RUNTIME_DIR"
mkdir "$work/labwc"
printf '%s\n' '<labwc_config/>' > "$work/labwc/rc.xml"
export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1
export LIBGL_ALWAYS_SOFTWARE=1 WGPU_BACKEND=vulkan GDK_BACKEND=wayland
# The renderer samples a depth texture, which wgpu's GLES backend cannot
# translate. Force Mesa's CPU Vulkan driver so this also works without a GPU.
for driver in /usr/share/vulkan/icd.d/lvp_icd*.json; do
    if [[ -f "$driver" ]]; then
        export VK_DRIVER_FILES="$driver" VK_ICD_FILENAMES="$driver"
        break
    fi
done
[[ -n "${VK_DRIVER_FILES:-}" ]] || { echo 'Mesa lavapipe Vulkan driver is required' >&2; exit 1; }
unset DISPLAY WAYLAND_DISPLAY
labwc --config-dir "$work/labwc" > "$output/compositor.log" 2>&1 &
compositor=$!
for ((attempt=0; attempt<100; attempt++)); do
    kill -0 "$compositor" 2>/dev/null || { cat "$output/compositor.log" >&2; exit 1; }
    for socket in "$XDG_RUNTIME_DIR"/wayland-*; do
        if [[ -S "$socket" ]]; then
            export WAYLAND_DISPLAY=${socket##*/}
            break 2
        fi
    done
    sleep 0.1
done
[[ -n "${WAYLAND_DISPLAY:-}" ]] || { echo 'Wayland socket did not appear' >&2; exit 1; }
timeout 180s python3 "$scripts/../validation/managed/smoke.py" "$binary" --output "$output" "${probe[@]}"
