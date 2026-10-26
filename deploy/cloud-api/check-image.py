#!/usr/bin/env python3
"""Checks of the image of the cloud API that need more than a shell line,
for the Container workflow; the standard library only.

    python3 -I check-image.py config CONFIG.json NUMBER REVISION
    python3 -I check-image.py log LOG FILES_DIR KEY
    python3 -I check-image.py notices THIRD-PARTY-NOTICES.txt

config  CONFIG.json is `docker image inspect --format '{{json .Config}}'`:
        the user 10001:10001, the health check, and the labels, with the
        version NUMBER and the commit REVISION.
log     LOG is the output of `docker logs` after the smoke test: it holds
        request lines, the prefix of the key's hash and neither the key nor
        any part of the base64 of the test files in FILES_DIR, which
        make-test-files.py wrote.
notices the notices name acadrust, which the drawings take, and not
        reqwest, which only the bag3d feature of the core brings, which the
        service leaves out.

Each prints a Markdown section for the job summary and exits 1 when a check
fails.
"""

import base64
import hashlib
import json
import pathlib
import sys

HEALTH_CHECK = ["CMD", "/usr/local/bin/open-pointcloud-studio-api", "--health-check"]
SOURCE = "https://github.com/OpenAEC-Foundation/open-pointcloud-studio"


class Section:
    def __init__(self, title):
        self.lines = [f"### {title}", "", "| Check | Result |", "| --- | --- |"]
        self.failures = 0

    def check(self, passed, text):
        if not passed:
            self.failures += 1
        print(("ok    " if passed else "WRONG ") + text, file=sys.stderr)
        self.lines.append("| %s | %s |" % (text.replace("|", "\\|"), "ok" if passed else "wrong"))

    def finish(self):
        print("\n".join(self.lines) + "\n")
        return 1 if self.failures else 0


def config(path, number, revision):
    with open(path, encoding="utf-8") as file:
        settings = json.load(file)
    section = Section("The image")
    labels = settings.get("Labels") or {}
    health = settings.get("Healthcheck") or {}
    section.check(settings.get("User") == "10001:10001", f"runs as user {settings.get('User')!r}, expected '10001:10001'")
    section.check(
        health.get("Test") == HEALTH_CHECK,
        f"health check `{' '.join(health.get('Test') or [])}`, every {health.get('Interval', 0) // 10**9} s",
    )
    section.check(settings.get("Entrypoint") == ["/usr/local/bin/open-pointcloud-studio-api"], "the service is the entry point")
    section.check("/work" in (settings.get("Volumes") or {}), "/work is a volume")
    environment = dict(entry.split("=", 1) for entry in settings.get("Env") or [])
    for name, value in (
        ("OPENAEC_MCP_HTTP", "0.0.0.0:8080"),
        ("OPENAEC_API_LOGVORM", "json"),
        ("OPENAEC_API_WERKMAP", "/work"),
        ("TMPDIR", "/work/tmp"),
    ):
        section.check(environment.get(name) == value, f"{name}={environment.get(name)}")
    section.check("OPENAEC_API_TOEGANG" not in environment, "the image does not set OPENAEC_API_TOEGANG")
    expected = {
        "org.opencontainers.image.title": "Open Pointcloud Studio API",
        "org.opencontainers.image.version": number,
        "org.opencontainers.image.revision": revision,
        "org.opencontainers.image.source": SOURCE,
        "org.opencontainers.image.documentation": SOURCE + "/blob/main/docs/cloud-api.md",
        "org.opencontainers.image.licenses": "LGPL-3.0-or-later",
        "org.opencontainers.image.vendor": "OpenAEC Foundation",
    }
    for name, value in expected.items():
        section.check(labels.get(name) == value, f"label {name} = {labels.get(name)}")
    section.check(bool(labels.get("org.opencontainers.image.description")), "label org.opencontainers.image.description is set")
    return section.finish()


def log(path, files, key):
    text = pathlib.Path(path).read_text(encoding="utf-8", errors="replace")
    section = Section("The log of the container")
    requests = [line for line in text.splitlines() if '"message":"request"' in line]
    section.check(len(requests) > 0, f"{len(requests)} request lines, one JSON object each")
    prefix = hashlib.sha256(key.encode("utf-8")).hexdigest()[:8]
    section.check(f'"key":"{prefix}"' in text, "requests name the first eight digits of the key's hash")
    section.check(key not in text, "the key is not in the log")
    section.check(hashlib.sha256(key.encode("utf-8")).hexdigest() not in text, "the whole hash of the key is not in the log")
    leaked = []
    for file in sorted(pathlib.Path(files).iterdir()):
        if file.suffix == ".json":
            continue
        encoded = base64.b64encode(file.read_bytes()).decode("ascii")
        for start in (0, len(encoded) // 2, max(0, len(encoded) - 48)):
            if encoded[start:start + 48] in text:
                leaked.append(file.name)
                break
    section.check(not leaked, "no base64 of the test files is in the log" + (f" (found: {', '.join(leaked)})" if leaked else ""))
    return section.finish()


def notices(path):
    text = pathlib.Path(path).read_text(encoding="utf-8")
    section = Section("The notices in the image")
    lines = text.splitlines()
    section.check(any(line.startswith("open-pointcloud-studio-api ") for line in lines[:5]), "they are those of open-pointcloud-studio-api")
    section.check(any(line.startswith("acadrust ") for line in lines), "they name acadrust")
    section.check("reqwest" not in text, "they do not name reqwest: the bag3d feature is off")
    return section.finish()


def main(arguments):
    commands = {"config": (config, 3), "log": (log, 3), "notices": (notices, 1)}
    if not arguments or arguments[0] not in commands or len(arguments) - 1 != commands[arguments[0]][1]:
        print(__doc__.strip().split("\n\n")[1], file=sys.stderr)
        return 2
    function, _ = commands[arguments[0]]
    return function(*arguments[1:])


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
