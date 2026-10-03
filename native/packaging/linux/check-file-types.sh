#!/usr/bin/env bash
# After the .deb is installed: does the system recognise the point-cloud
# files, and does it offer the application for them?
#
#   check-file-types.sh FOLDER
#
# FOLDER is what smoke-test.sh left with SMOKE_KEEP_DIR. Formats that the
# application reads but does not write get a minimal file here.
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/../common.sh"

folder=${1:?usage: check-file-types.sh FOLDER}
samples=$folder/types
mkdir -p "$samples"

cp "$folder/scans/grid.ply" "$folder/out/grid.e57" "$folder/out/grid.las" \
    "$folder/out/grid.laz" "$folder/out/grid.pts" "$samples/"
printf '# .PCD v0.7 - Point Cloud Data file format\nVERSION 0.7\nFIELDS x y z\nSIZE 4 4 4\nTYPE F F F\nCOUNT 1 1 1\nWIDTH 1\nHEIGHT 1\nVIEWPOINT 0 0 0 1 0 0 0\nPOINTS 1\nDATA ascii\n0 0 0\n' > "$samples/grid.pcd"
printf '1\n1\n0 0 0\n1 0 0\n0 1 0\n0 0 1\n1 0 0 0\n0 1 0 0\n0 0 1 0\n0 0 0 1\n0 0 0 0.5\n' > "$samples/grid.ptx"
# Only the name decides for a scan project; the content is a ZIP container.
printf 'PK\003\004' > "$samples/project.rcp"

status=0
expect() {
    local file=$1 type=$2 found offered
    found=$(gio info -a standard::content-type "$samples/$file" | sed -n 's/.*standard::content-type: //p')
    if [[ "$found" == "$type" ]]; then
        echo "ok    $file is $type"
    else
        echo "WRONG $file is '$found', expected $type"
        status=1
    fi
    # The whole answer is read before it is searched. A search that stops at
    # the first match closes the pipe while gio still writes its lists, and
    # the failed write would count as "not offered".
    offered=$(gio mime "$type" 2>&1) || true
    if grep -qF "$APP_ID.desktop" <<< "$offered"; then
        echo "ok    $type is offered $APP_ID.desktop"
    else
        echo "WRONG $type is not offered $APP_ID.desktop:"
        sed 's/^/        /' <<< "$offered"
        status=1
    fi
}

expect grid.e57 model/e57
expect grid.las application/vnd.las
expect grid.laz application/vnd.laszip
expect grid.ply application/x-ply
expect grid.pcd application/x-pcd
expect grid.ptx application/x-ptx
expect grid.pts application/x-pts
expect project.rcp application/x-rcp

# A point-cloud file without a telling name is recognised by its first bytes.
cp "$samples/grid.e57" "$samples/unnamed-e57"
found=$(gio info -a standard::content-type "$samples/unnamed-e57" | sed -n 's/.*standard::content-type: //p')
if [[ "$found" == "model/e57" ]]; then
    echo "ok    an E57 file without extension is model/e57"
else
    echo "WRONG an E57 file without extension is '$found'"
    status=1
fi

exit $status
