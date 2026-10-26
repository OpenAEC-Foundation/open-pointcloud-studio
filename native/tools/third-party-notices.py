#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-3.0-or-later
"""Write the third-party notices of one package of the workspace.

    python3 -I third-party-notices.py --manifest-path native/Cargo.toml \\
        --package NAME --target TRIPLE [--target TRIPLE ...] --output FILE

The notices name every crate that the package links for the given targets:
the normal dependencies, as `cargo tree -e normal --target TRIPLE` lists
them, without the members of the workspace. With several targets (the two
halves of a universal macOS library) they hold the crates of all of them.
Each crate comes with its version, licence expression and repository, and
the text of its LICENSE, LICENCE, COPYING and NOTICE files. A crate that
ships no such file gets a reference to the standard text of each licence it
names.

Run it after `cargo build` with CARGO_NET_OFFLINE=true: the crates are read
from the registry sources that the build downloaded, under
$CARGO_HOME/registry/src, and a crate that was not downloaded for the build
makes Cargo fail, which proves that the notices list only what was compiled.

The script refuses a licence that a person has not looked at yet: every
crate must be usable under licences of ALLOWED (an expression `A OR B` needs
one of them, `A AND B` both). It uses the standard library only.

OPS_NOTICES_CARGO, when set, is the command that runs Cargo, split like a
shell line; the tests of the script put a stand-in there.
"""

import argparse
import json
import os
import re
import shlex
import subprocess
import sys
from pathlib import Path

# The licences of the crates that have been looked at. A new one fails the
# script, so that a person decides whether the library may carry it.
ALLOWED = {
    "MIT",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Zlib",
    "Unicode-3.0",
    "MPL-2.0",
    "BSL-1.0",
    "CC0-1.0",
    "CDLA-Permissive-2.0",
}
# The exceptions that may follow WITH.
EXCEPTIONS = {"LLVM-exception"}

# The files of a crate that hold its licence or notices, by the start of
# their name in any case.
LICENCE_FILES = ("license", "licence", "copying", "notice")

RULE = "=" * 78
THIN_RULE = "-" * 78


class NoticeError(Exception):
    """A reason the notices cannot be written."""


def cargo_command():
    configured = os.environ.get("OPS_NOTICES_CARGO")
    return shlex.split(configured) if configured else ["cargo"]


def run_cargo(arguments):
    command = cargo_command() + arguments
    result = subprocess.run(command, capture_output=True, text=True, encoding="utf-8")
    if result.returncode != 0:
        raise NoticeError(
            "cargo %s failed:\n%s" % (" ".join(arguments), result.stderr.strip())
        )
    return result.stdout


# ---- licence expressions -------------------------------------------------


def tokens(expression):
    """The words and parentheses of an expression; the old form "MIT/Apache-2.0"
    of crates.io reads as an OR."""
    found = []
    for token in re.findall(r"\(|\)|[^\s()]+", expression):
        if "/" in token and token != "/":
            parts = token.split("/")
            for index, part in enumerate(parts):
                if index:
                    found.append("OR")
                if part:
                    found.append(part)
        else:
            found.append(token)
    return found


class Expression:
    """A parsed licence expression: an identifier, `X WITH E`, or an AND or
    OR of expressions."""

    def __init__(self, kind, parts=(), name=None, exception=None):
        self.kind = kind
        self.parts = list(parts)
        self.name = name
        self.exception = exception

    def acceptable(self):
        if self.kind == "id":
            return self.name in ALLOWED and (
                self.exception is None or self.exception in EXCEPTIONS
            )
        if self.kind == "and":
            return all(part.acceptable() for part in self.parts)
        return any(part.acceptable() for part in self.parts)

    def identifiers(self):
        if self.kind == "id":
            return [self.name] + ([self.exception] if self.exception else [])
        found = []
        for part in self.parts:
            found.extend(part.identifiers())
        return found


def parse_expression(expression):
    words = tokens(expression)
    position = 0

    def peek():
        return words[position] if position < len(words) else None

    def take():
        nonlocal position
        word = peek()
        if word is None:
            raise NoticeError("licence expression '%s' ends too early" % expression)
        position += 1
        return word

    def operator(word, name):
        return word is not None and word.upper() == name

    def either():
        parts = [both()]
        while operator(peek(), "OR"):
            take()
            parts.append(both())
        return parts[0] if len(parts) == 1 else Expression("or", parts)

    def both():
        parts = [single()]
        while operator(peek(), "AND"):
            take()
            parts.append(single())
        return parts[0] if len(parts) == 1 else Expression("and", parts)

    def single():
        word = take()
        if word == "(":
            inner = either()
            if take() != ")":
                raise NoticeError("licence expression '%s' misses a ')'" % expression)
            return inner
        if word == ")" or operator(word, "AND") or operator(word, "OR") or operator(word, "WITH"):
            raise NoticeError("licence expression '%s' has '%s' out of place" % (expression, word))
        if operator(peek(), "WITH"):
            take()
            exception = take()
            if exception in ("(", ")"):
                raise NoticeError("licence expression '%s' has no exception after WITH" % expression)
            return Expression("id", name=word, exception=exception)
        return Expression("id", name=word)

    if not words:
        raise NoticeError("the licence expression is empty")
    parsed = either()
    if position != len(words):
        raise NoticeError(
            "licence expression '%s' has '%s' out of place" % (expression, words[position])
        )
    return parsed


# ---- the crates ------------------------------------------------------------


def workspace_packages(manifest):
    """The members of the workspace: name to version."""
    metadata = json.loads(
        run_cargo(
            ["metadata", "--no-deps", "--format-version", "1", "--locked", "--manifest-path", manifest]
        )
    )
    return {package["name"]: package["version"] for package in metadata["packages"]}


PACKAGE = re.compile(r"^(?P<name>\S+) v(?P<version>\S+)(?P<rest>.*)$")


def crates_for(manifest, package, target):
    """The crates `cargo tree` lists for one target: (name, version) to
    (licence, repository, source note)."""
    output = run_cargo(
        [
            "tree",
            "-p",
            package,
            "-e",
            "normal",
            "--target",
            target,
            "--prefix",
            "none",
            "--locked",
            "-f",
            "{p}|{l}|{r}",
            "--manifest-path",
            manifest,
        ]
    )
    crates = {}
    for line in output.splitlines():
        line = line.strip()
        if not line:
            continue
        # A crate listed before is marked, and its dependencies left out.
        if line.endswith("(*)"):
            line = line[: -len("(*)")].rstrip()
        fields = line.split("|", 2)
        if len(fields) != 3:
            raise NoticeError("cannot read the line '%s' of cargo tree" % line)
        described, licence, repository = (field.strip() for field in fields)
        match = PACKAGE.match(described)
        if not match:
            raise NoticeError("cannot read the package '%s' of cargo tree" % described)
        rest = match.group("rest").replace("(proc-macro)", "").strip()
        crates[(match.group("name"), match.group("version"))] = (licence, repository, rest)
    return crates


def version_key(version):
    return [(0, int(part)) if part.isdigit() else (1, part) for part in re.split(r"[.+-]", version)]


def cargo_home():
    configured = os.environ.get("CARGO_HOME")
    return Path(configured) if configured else Path.home() / ".cargo"


def crate_source(name, version):
    sources = sorted((cargo_home() / "registry" / "src").glob("*/%s-%s" % (name, version)))
    sources = [source for source in sources if source.is_dir()]
    if not sources:
        raise NoticeError(
            "the source of %s %s is not under %s; build the package first"
            % (name, version, cargo_home() / "registry" / "src")
        )
    return sources[0]


def licence_texts(source):
    found = []
    for path in sorted(source.iterdir(), key=lambda path: path.name):
        if path.is_file() and path.name.lower().startswith(LICENCE_FILES):
            text = path.read_text(encoding="utf-8", errors="replace")
            text = text.replace("\r\n", "\n").replace("\r", "\n").strip("\n")
            found.append((path.name, text))
    return found


def notices(manifest, package, targets, members):
    """The entries of the notices, sorted by name and version."""
    crates = {}
    for target in targets:
        crates.update(crates_for(manifest, package, target))
    problems = []
    entries = []
    for (name, version) in sorted(crates, key=lambda key: (key[0], version_key(key[1]))):
        if members.get(name) == version:
            continue
        licence, repository, rest = crates[(name, version)]
        if rest:
            problems.append("%s %s does not come from a registry: %s" % (name, version, rest))
            continue
        if not licence:
            problems.append("%s %s states no licence" % (name, version))
            continue
        try:
            expression = parse_expression(licence)
        except NoticeError as error:
            problems.append("%s %s: %s" % (name, version, error))
            continue
        if not expression.acceptable():
            problems.append(
                "%s %s is licensed '%s', which is not among the licences looked at: %s"
                % (name, version, licence, ", ".join(sorted(ALLOWED)))
            )
            continue
        try:
            texts = licence_texts(crate_source(name, version))
        except NoticeError as error:
            problems.append(str(error))
            continue
        entries.append((name, version, licence, repository, texts, expression))
    if problems:
        raise NoticeError("\n".join(problems))
    return entries


def write(output, package, version, targets, entries):
    lines = [
        "THIRD-PARTY NOTICES",
        "",
        "%s %s" % (package, version),
        "Built for: %s" % ", ".join(targets),
        "",
        "It is built from the following Rust crates, each one unmodified and under",
        "its own licence; a few of them only generate code while it is compiled.",
        "Their source code is available from crates.io under the name and version",
        "given. The licence texts that the crates ship follow the list; a crate",
        "that ships none is covered by the standard text of each licence it names.",
        "",
    ]
    width = max((len(name) for name, *_ in entries), default=0)
    for name, version, licence, *_ in entries:
        lines.append("%-*s  %-12s  %s" % (width, name, version, licence))
    for name, version, licence, repository, texts, expression in entries:
        lines += ["", RULE, "%s %s" % (name, version), "Licence: %s" % licence]
        if repository:
            lines.append("Repository: %s" % repository)
        if texts:
            for file_name, text in texts:
                lines += [THIN_RULE, file_name, "", text]
        else:
            seen = []
            for identifier in expression.identifiers():
                if identifier not in seen:
                    seen.append(identifier)
            for identifier in seen:
                lines.append("Standard text: https://spdx.org/licenses/%s.html" % identifier)
    text = "\n".join(lines) + "\n"
    Path(output).parent.mkdir(parents=True, exist_ok=True)
    with open(output, "w", encoding="utf-8", newline="\n") as file:
        file.write(text)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--manifest-path", required=True)
    parser.add_argument("--package", required=True)
    parser.add_argument("--target", action="append", required=True)
    parser.add_argument("--output", required=True)
    arguments = parser.parse_args()
    targets = sorted(set(arguments.target))
    try:
        members = workspace_packages(arguments.manifest_path)
        if arguments.package not in members:
            raise NoticeError("the workspace has no package %s" % arguments.package)
        entries = notices(arguments.manifest_path, arguments.package, targets, members)
        write(arguments.output, arguments.package, members[arguments.package], targets, entries)
    except NoticeError as error:
        print("third-party-notices.py: %s" % error, file=sys.stderr)
        return 1
    print("%s: %d crates" % (arguments.output, len(entries)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
