#!/usr/bin/env python3
"""Check every linearctl invocation in a Markdown doc against `linearctl describe`.

The design names the exact commands the TUI will spawn. This keeps those names
honest: each `linearctl ...` in an inline code span or a fenced code block must
resolve to a real command, and each --flag must exist on it (or be global).

Invocations under a heading that contains "(proposed)" are exempt: they are
the gaps we ask linearctl to fill, so they do not exist yet.

Usage:
    check_contract.py DOC [--describe PATH | --live [BINARY]]

--describe reads a saved `linearctl describe --json` (the default is the
snapshot in tests/fixtures); --live runs the binary instead.
"""

import json
import re
import shlex
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SNAPSHOT = ROOT / "tests" / "fixtures" / "linearctl-describe-0.1.0.json"

FENCE = re.compile(r"^\s*(```|~~~)")
HEADING = re.compile(r"^(#{1,6})\s+(.*)$")
INLINE = re.compile(r"`([^`]+)`")


def invocations(markdown):
    """Yield (line number, command text) for each linearctl call in the doc."""
    in_fence = False
    exempt_level = None  # heading level of the "(proposed)" section we are in
    for n, line in enumerate(markdown.splitlines(), 1):
        if FENCE.match(line):
            in_fence = not in_fence
            continue
        if not in_fence:
            h = HEADING.match(line)
            if h:
                level = len(h.group(1))
                if exempt_level is not None and level <= exempt_level:
                    exempt_level = None
                if exempt_level is None and "(proposed)" in h.group(2):
                    exempt_level = level
                continue
        if exempt_level is not None:
            continue
        spans = [line] if in_fence else INLINE.findall(line)
        for span in spans:
            # A span may hold a pipeline or a sequence; check each linearctl part.
            for part in re.split(r"\||;|&&|→", span):
                part = part.strip()
                if part.startswith("linearctl ") or part == "linearctl":
                    yield n, part


def index(describe):
    """Map command paths ("issue show", "show" via alias, ...) to their flags."""
    global_flags = {f["name"] for f in describe["flags"]}
    table = {}

    def walk(commands, path, inherited):
        for c in commands:
            flags = inherited | {f["name"] for f in c["flags"]}
            for name in [c["name"], *c["aliases"]]:
                p = path + (name,)
                table[p] = (flags, bool(c["commands"]))
                walk(c["commands"], p, flags)

    walk(describe["commands"], (), global_flags)
    return table, global_flags


def check_one(text, table, global_flags):
    """Return a list of problems with one invocation; empty means it is valid."""
    try:
        argv = shlex.split(text)
    except ValueError as e:
        return [f"cannot parse: {e}"]
    argv = argv[1:]  # drop "linearctl"
    path = ()
    i = 0
    while i < len(argv) and path + (argv[i],) in table:
        path += (argv[i],)
        i += 1
    if i < len(argv) and argv[i] in ("…", "..."):
        return []  # prose placeholder: "linearctl … --json" names no command
    if not path:
        flags = global_flags
        if i < len(argv) and not argv[i].startswith("-"):
            return [f"unknown command: {argv[i]}"]
    else:
        flags, has_children = table[path]
        if has_children:
            nxt = argv[i] if i < len(argv) else ""
            if nxt and not nxt.startswith("-"):
                return [f"unknown command: {' '.join(path)} {nxt}"]
    problems = []
    for tok in argv[i:]:
        tok = tok.strip("[]()")  # synopsis brackets: [--team KEY]
        if not tok.startswith("--"):
            continue
        name = tok[2:].split("=", 1)[0]
        if name and name not in flags:
            problems.append(f"unknown flag --{name} on '{' '.join(('linearctl',) + path)}'")
    return problems


def check(markdown, describe):
    table, global_flags = index(describe)
    found = 0
    problems = []
    for n, text in invocations(markdown):
        found += 1
        for p in check_one(text, table, global_flags):
            problems.append(f"line {n}: {p}: {text}")
    return found, problems


def load_describe(args):
    if "--live" in args:
        i = args.index("--live")
        binary = args[i + 1] if i + 1 < len(args) else "linearctl"
        out = subprocess.run([binary, "describe", "--json"], check=True,
                             capture_output=True, text=True).stdout
        return json.loads(out)
    if "--describe" in args:
        return json.loads(Path(args[args.index("--describe") + 1]).read_text())
    return json.loads(SNAPSHOT.read_text())


def main(args):
    if not args or args[0] in ("-h", "--help"):
        print(__doc__.strip())
        return 0 if args else 2
    doc = Path(args[0])
    found, problems = check(doc.read_text(), load_describe(args[1:]))
    for p in problems:
        print(f"{doc}: {p}", file=sys.stderr)
    if found == 0:
        print(f"{doc}: no linearctl invocations found", file=sys.stderr)
        return 1
    print(f"{doc}: {found} linearctl invocations, {len(problems)} problems")
    return 1 if problems else 0


if __name__ == "__main__":  # pragma: no cover
    sys.exit(main(sys.argv[1:]))
