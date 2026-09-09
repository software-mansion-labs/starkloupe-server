#!/usr/bin/env python3
"""Read and rewrite the starknet-foundry pin in Cargo.toml.

The workspace pins every starknet-foundry crate at one point in upstream
history, spread over several dependency entries that must always agree. Cargo
accepts two ways of naming that point and both are in use here:

    rev = "<40 hex>"   what the canary tests against - the tip of master
    tag = "<tag>"      what a release bump commits - a full release

So the pin cannot be found by looking for one of them. This script is the one
place that knows about both, used by .github/workflows/bump-starknet-foundry.yml
(writes a rev) and bump-starknet-foundry-release.yml (writes a tag).

Cargo's git locators - branch, tag and rev - are mutually exclusive, so each
crate is checked to carry exactly one. A leftover `branch = "master"` beside a
rev is rejected rather than silently outvoted by whichever the pin happens to
match.

Reading goes through tomllib, so a commented-out dependency or a reformatted
manifest cannot be mistaken for the real pin. Writing is a targeted line edit
rather than a re-serialisation, because there is no format-preserving writer in
the standard library and re-serialising would drop every comment in Cargo.toml.
The edit is verified by parsing the result and checking the pin reads back as
intended, so a miss fails loudly instead of corrupting the manifest.

Usage:
    scripts/foundry_pin.py read          # pin_key=tag\\npin_value=v0.63.0
    scripts/foundry_pin.py set rev 5eb423a9cbe9feefbf94a5cdb3a0288a09b5848b
    scripts/foundry_pin.py set tag v0.63.0

`read` prints GitHub Actions `key=value` output lines, so a workflow step can
append it straight to $GITHUB_OUTPUT.
"""

import argparse
import re
import tomllib
from pathlib import Path

DEFAULT_MANIFEST = Path(__file__).resolve().parent.parent / "Cargo.toml"

# The dependency tables a git dependency can legally appear in. This manifest is
# a virtual workspace root, so in practice everything is in the first one.
DEP_TABLES = (
    ("workspace", "dependencies"),
    ("dependencies",),
    ("dev-dependencies",),
    ("build-dependencies",),
)

# The three ways a git dependency can name a point in history. Cargo rejects
# more than one, so anything that carries a second is a manifest bug, not a pin
# this script gets to choose between. Only rev and tag are ever written.
LOCATOR_KEYS = ("branch", "tag", "rev")
WRITABLE_KEYS = ("rev", "tag")

# Used only to edit a line already identified through the parsed document. It
# covers branch as well, so a floating branch entry converts to a pinned one.
PIN_RE = re.compile(r'\b(branch|tag|rev)\b(\s*=\s*)"([^"]*)"')
REV_RE = re.compile(r"^[0-9a-f]{40}$")


def find_deps(manifest, document):
    """{name: spec} for every dependency pinned to a starknet-foundry remote.

    Matches on the remote rather than a fixed URL, so the pin is still found if
    the deps are ever repointed at a fork.
    """
    deps = {}
    for path in DEP_TABLES:
        table = document
        for key in path:
            table = table.get(key, {}) if isinstance(table, dict) else {}
        for name, spec in table.items():
            if isinstance(spec, dict) and "starknet-foundry" in spec.get("git", ""):
                deps[name] = spec
    if not deps:
        raise SystemExit(f"{manifest}: no starknet-foundry dependency found")
    return deps


def read_pin(manifest, document):
    """The single (key, value) every starknet-foundry dependency agrees on.

    Also the post-write check: it fails if an edit left a second locator behind.
    """
    pins = set()
    for name, spec in sorted(find_deps(manifest, document).items()):
        present = [key for key in LOCATOR_KEYS if key in spec]
        if not present:
            raise SystemExit(
                f"{manifest}: {name} sets none of {', '.join(LOCATOR_KEYS)}"
            )
        if len(present) > 1:
            conflict = ", ".join(f'{k} = "{spec[k]}"' for k in present)
            raise SystemExit(
                f"{manifest}: {name} sets conflicting git locators: {conflict}"
            )
        pins.add((present[0], spec[present[0]]))
    if len(pins) > 1:
        found = ", ".join(f'{k} = "{v}"' for k, v in sorted(pins))
        raise SystemExit(
            f"{manifest}: starknet-foundry crates disagree on the pin: {found}"
        )
    return pins.pop()


def pin_line(lines, name, start=0):
    """Index of the line holding `name`'s pin.

    The entry is normally one line - `cheatnet = { git = "...", rev = "..." }` -
    so the pin sits on the line that opens it. A multi-line inline table or a
    [workspace.dependencies.cheatnet] section puts it further down, so the
    search continues to the next entry or the end of the table.
    """
    opener = re.compile(rf"^\s*(?:\[[^]]*\.)?{re.escape(name)}\s*[=\]]")
    for i in range(start, len(lines)):
        if not opener.match(lines[i]):
            continue
        for j in range(i, len(lines)):
            if PIN_RE.search(lines[j]):
                return j
            # Stop at a blank line or the start of the next entry or section.
            if j > i and (not lines[j].strip() or lines[j].lstrip().startswith("[")):
                break
        break
    return None


def write_pin(manifest, text, document, key, value):
    if key == "rev" and not REV_RE.match(value):
        raise SystemExit(f"not a full 40-character commit sha: {value}")
    if key == "tag" and (not value or '"' in value):
        raise SystemExit(f"not a usable tag: {value!r}")

    deps = find_deps(manifest, document)
    lines = text.splitlines(keepends=True)

    # Replaces whichever of rev/tag is there, so a rev pin can become a tag pin
    # and back without either workflow caring which it started from.
    edited = set()
    for name in deps:
        i = pin_line(lines, name)
        if i is None:
            raise SystemExit(f"{manifest}: cannot locate the pin for {name}")
        lines[i] = PIN_RE.sub(f'{key}\\g<2>"{value}"', lines[i], count=1)
        edited.add(i)

    # The edit was line surgery, so confirm against a real parse that it landed
    # where it was meant to and nowhere else, before anything is written out.
    updated = "".join(lines)
    try:
        reparsed = tomllib.loads(updated)
    except tomllib.TOMLDecodeError as exc:
        raise SystemExit(f"{manifest}: edit produced invalid TOML: {exc}") from exc

    if set(find_deps(manifest, reparsed)) != set(deps):
        raise SystemExit(f"{manifest}: edit changed which crates are pinned")
    if read_pin(manifest, reparsed) != (key, value):
        raise SystemExit(f"{manifest}: edit did not apply cleanly")
    changed = sum(1 for a, b in zip(text.splitlines(), updated.splitlines()) if a != b)
    if changed != len(edited):
        raise SystemExit(f"{manifest}: edit touched {changed} lines, expected {len(edited)}")

    manifest.write_text(updated)
    return len(deps)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("read")
    write = sub.add_parser("set")
    write.add_argument("key", choices=WRITABLE_KEYS)
    write.add_argument("value")
    args = parser.parse_args()

    text = args.manifest.read_text()
    try:
        document = tomllib.loads(text)
    except tomllib.TOMLDecodeError as exc:
        raise SystemExit(f"{args.manifest}: {exc}") from exc

    if args.command == "read":
        key, value = read_pin(args.manifest, document)
        print(f"pin_key={key}")
        print(f"pin_value={value}")
    else:
        count = write_pin(args.manifest, text, document, args.key, args.value)
        print(f"pinned {count} starknet-foundry crates at {args.key} = {args.value}")


if __name__ == "__main__":
    main()
