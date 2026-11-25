#!/usr/bin/env python3
"""Check or stage Classic product versions without renumbering dependencies."""

import argparse
import json
import re
from pathlib import Path


FILES = (
    "classic/package.json",
    "classic/package-lock.json",
    "classic/src-tauri/Cargo.toml",
    "classic/src-tauri/Cargo.lock",
    "classic/src-tauri/tauri.conf.json",
)


def contents(root, number):
    if not (root / "classic").exists():
        return {}  # Native-only consumers and the packaging-script fixtures.
    major, week, sequence = map(int, number.split("."))
    if not 2000 <= major <= 2255:
        raise ValueError("Classic MSI product years must be between 2000 and 2255")
    result = {}
    for name in FILES:
        path = root / name
        text = path.read_text()
        if name.endswith(".json"):
            data = json.loads(text)
            data["version"] = number
            if name.endswith("package-lock.json"):
                data["packages"][""]["version"] = number
            elif name.endswith("tauri.conf.json"):
                # MSI's major field cannot hold the four-digit release year.
                data["bundle"].setdefault("windows", {}).setdefault("wix", {})[
                    "version"
                ] = f"{major - 2000}.{week}.{sequence}"
            text = json.dumps(data, indent=2, ensure_ascii=False) + "\n"
        else:
            section = "[[package]]" if name.endswith("Cargo.lock") else "[package]"
            blocks = text.split(section)
            matched = 0
            for index in range(1, len(blocks)):
                block = blocks[index]
                if re.search(r'^name\s*=\s*"open-pointcloud-studio"\s*$', block, re.M):
                    blocks[index], count = re.subn(
                        r'^(version\s*=\s*)"[^"\n]+"',
                        lambda match: match[1] + json.dumps(number),
                        block,
                        count=1,
                        flags=re.M,
                    )
                    matched += count
            if matched != 1:
                raise ValueError(f"{name}: expected one product package version")
            text = section.join(blocks)
        result[name] = text
    return result


def check(root, number):
    for name, expected in contents(root, number).items():
        # Compare the relevant parsed fields, without requiring JSON formatting.
        actual = (root / name).read_text()
        if name.endswith(".json"):
            left, right = json.loads(actual), json.loads(expected)
            if left != right:
                raise ValueError(f"{name}: product version does not match {number}")
        elif actual != expected:
            raise ValueError(f"{name}: product version does not match {number}")
        print(f"{name}: {number}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("number")
    parser.add_argument("--stage", type=Path)
    args = parser.parse_args()
    try:
        if args.stage is None:
            check(args.root, args.number)
        else:
            # Parse every source before staging anything; callers publish only
            # after all staging and changelog checks have succeeded.
            for name, text in contents(args.root, args.number).items():
                target = args.stage / name
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(text)
                print(name)
    except (ValueError, KeyError, OSError) as error:
        parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    main()
