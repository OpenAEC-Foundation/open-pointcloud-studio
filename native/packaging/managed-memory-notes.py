"""Append the measured idle RSS of both Linux AppImages to the release notes."""

import json
import sys
from pathlib import Path


def idle_rss_kib(path, version):
    report = json.loads(path.read_text(encoding="utf-8"))
    if (
        report["ok"] is not True
        or report["mcp_paused"] is not False
        or report["server"]["version"] != version
    ):
        raise ValueError(f"{path}: expected a successful managed-window test for {version}")
    kib = report["idle_rss_kib"]
    if type(kib) is not int or kib <= 0:
        raise ValueError(f"{path}: expected positive idle_rss_kib")
    return kib


def main(version, directory):
    rows = [
        (arch, idle_rss_kib(directory / f"managed-linux-{arch}" / "result.json", version))
        for arch in ("amd64", "arm64")
    ]
    print("\n## Linux AppImage")
    print("\nResident memory of the empty application window with Wayland and software rendering.")
    print("Resident geheugen van de applicatie met een leeg venster onder Wayland met softwarerendering.")
    print("\n| Architecture / Architectuur | RSS (MiB) |")
    print("| --- | ---: |")
    for arch, kib in rows:
        print(f"| {arch} | {kib / 1024:.1f} |")


if __name__ == "__main__":
    try:
        main(sys.argv[1], Path(sys.argv[2]))
    except (OSError, KeyError, ValueError) as error:
        sys.exit(f"Managed AppImage measurement: {error}")
