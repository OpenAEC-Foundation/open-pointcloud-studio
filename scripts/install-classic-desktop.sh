#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
desktop_entry="${XDG_DATA_HOME:-$HOME/.local/share}/applications/open-pointcloud-studio-classic.desktop"
icon="$project_dir/classic/dist/extracted/usr/share/icons/hicolor/128x128/apps/open-pointcloud-studio.png"

if [[ ! -x "$project_dir/classic/dist/extracted/usr/bin/open-pointcloud-studio" ]]; then
  echo "Classic desktop binary is missing. Run './scripts/classic.sh fetch' first." >&2
  exit 1
fi

mkdir -p "$(dirname "$desktop_entry")"
cat > "$desktop_entry" <<EOF
[Desktop Entry]
Type=Application
Name=Open Pointcloud Studio Classic
Comment=Original v0.3.0 pointcloud desktop application
Exec=$project_dir/scripts/classic.sh run
Icon=$icon
Terminal=false
Categories=Graphics;Science;
EOF
echo "$desktop_entry"
