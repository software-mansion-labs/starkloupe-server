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

The manifest is edited through tomlkit, which keeps comments and formatting, so
a bump touches only the values it moves. The result is then parsed again with
tomllib and checked to read back as intended before it is written out.

Moving the pin is not enough on its own: the workspace also depends directly
on crates foundry pins exactly - cairo-lang-* at `=x.y.z`, blockifier and
starknet_api at release candidates - and a foundry release that bumps them
fails to resolve against ours. `sync-versions` takes foundry's own Cargo.toml
at the new pin and copies its version requirements onto the dependencies the
two workspaces share where either side pins rigidly - an exact `=` requirement
or a prerelease, which Cargo will not unify with anything else - plus the whole
cairo-lang-* family, which has members here that foundry does not depend on
and that must still move in lockstep. Ordinary caret requirements are left
alone: Cargo either unifies them or builds both majors side by side, so they
never block resolution, and moving them would be an unrelated upgrade.

Requires tomlkit (`pip install tomlkit==0.13.3`).

Usage:
    scripts/foundry_pin.py read          # pin_key=tag\\npin_value=v0.63.0
    scripts/foundry_pin.py set rev 5eb423a9cbe9feefbf94a5cdb3a0288a09b5848b
    scripts/foundry_pin.py set tag v0.63.0
    scripts/foundry_pin.py sync-versions path/to/foundry/Cargo.toml

`read` prints GitHub Actions `key=value` output lines, so a workflow step can
append it straight to $GITHUB_OUTPUT.
"""

import argparse
import re
import tomllib
from pathlib import Path

import tomlkit

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

REV_RE = re.compile(r"^[0-9a-f]{40}$")
# Crates released together from the Cairo compiler repo. They have to agree on
# one version, so all of them follow foundry, including ones it does not use.
CAIRO_FAMILY = "cairo-lang-"


def parse(path):
    try:
        return tomlkit.parse(path.read_text())
    except tomlkit.exceptions.ParseError as exc:
        raise SystemExit(f"{path}: {exc}") from exc


def dep_tables(document, tables=DEP_TABLES):
    for path in tables:
        table = document
        for key in path:
            table = table.get(key, {}) if isinstance(table, dict) else {}
        yield table


def find_deps(manifest, document):
    """{name: spec} for every dependency pinned to a starknet-foundry remote.

    Matches on the remote rather than a fixed URL, so the pin is still found if
    the deps are ever repointed at a fork.
    """
    deps = {}
    for table in dep_tables(document):
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
        pins.add((present[0], str(spec[present[0]])))
    if len(pins) > 1:
        found = ", ".join(f'{k} = "{v}"' for k, v in sorted(pins))
        raise SystemExit(
            f"{manifest}: starknet-foundry crates disagree on the pin: {found}"
        )
    return pins.pop()


def reparse(manifest, document):
    """The edited manifest as an independent parser sees it."""
    try:
        return tomllib.loads(tomlkit.dumps(document))
    except tomllib.TOMLDecodeError as exc:
        raise SystemExit(f"{manifest}: edit produced invalid TOML: {exc}") from exc


def write_pin(manifest, document, key, value):
    if key == "rev" and not REV_RE.match(value):
        raise SystemExit(f"not a full 40-character commit sha: {value}")
    if key == "tag" and not value:
        raise SystemExit("not a usable tag: ''")

    deps = find_deps(manifest, document)
    # read_pin has already guaranteed each crate carries exactly one locator.
    current, _ = read_pin(manifest, document)

    # Replaces whichever locator is there, so a rev pin can become a tag pin and
    # back without either workflow caring which it started from.
    for spec in deps.values():
        if current == key:
            spec[key] = value
        else:
            # tomlkit has no public way to rename a key in place: deleting and
            # re-adding moves it to the end of the table, and rebuilding the
            # table drops its spacing. Container._replace keeps both, which is
            # why tomlkit is pinned to an exact version in the workflows.
            spec.value._replace(current, key, tomlkit.item(value))

    reparsed = reparse(manifest, document)
    if set(find_deps(manifest, reparsed)) != set(deps):
        raise SystemExit(f"{manifest}: edit changed which crates are pinned")
    if read_pin(manifest, reparsed) != (key, value):
        raise SystemExit(f"{manifest}: edit did not apply cleanly")

    manifest.write_text(tomlkit.dumps(document))
    return len(deps)


def version_of(spec):
    """The version requirement of a dependency spec, None for git/path deps."""
    if isinstance(spec, str):
        return str(spec)
    if isinstance(spec, dict) and "version" in spec and "workspace" not in spec:
        return str(spec["version"])
    return None


def versioned_deps(document, tables=DEP_TABLES):
    deps = {}
    for table in dep_tables(document, tables):
        for name, spec in table.items():
            version = version_of(spec)
            if version is not None:
                deps[name] = version
    return deps


def is_rigid(requirement):
    """An exact or prerelease requirement - one Cargo will not float to unify."""
    return requirement.lstrip().startswith("=") or "-" in requirement


def wanted_versions(document, foundry):
    """{name: (current, wanted)} for every dependency that has to follow foundry."""
    ours = versioned_deps(document)
    theirs = versioned_deps(foundry, (("workspace", "dependencies"),))

    cairo = {v for name, v in theirs.items() if name.startswith(CAIRO_FAMILY)}
    if len(cairo) > 1:
        raise SystemExit(f"foundry pins conflicting {CAIRO_FAMILY}* versions: {sorted(cairo)}")

    wanted = {}
    for name, current in ours.items():
        if name in theirs:
            target = theirs[name]
            if not (is_rigid(current) or is_rigid(target)):
                continue
        elif name.startswith(CAIRO_FAMILY) and cairo:
            target = next(iter(cairo))
        else:
            continue
        if target != current:
            wanted[name] = (current, target)
    return wanted


def sync_versions(manifest, document, foundry):
    before = versioned_deps(document)
    pin = read_pin(manifest, document)
    wanted = wanted_versions(document, foundry)

    for table in dep_tables(document):
        for name in table:
            if name not in wanted:
                continue
            _, target = wanted[name]
            if isinstance(table[name], dict):
                table[name]["version"] = target
            else:
                # Assigning over a bare string keeps its trailing comment.
                table[name] = target

    # Every dependency must read back at exactly the version intended, and
    # nothing else - the starknet-foundry pin above all - may have moved.
    reparsed = reparse(manifest, document)
    expected = before | {name: target for name, (_, target) in wanted.items()}
    if versioned_deps(reparsed) != expected:
        raise SystemExit(f"{manifest}: version sync did not apply cleanly")
    if read_pin(manifest, reparsed) != pin:
        raise SystemExit(f"{manifest}: version sync touched the starknet-foundry pin")

    manifest.write_text(tomlkit.dumps(document))
    return wanted


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("read")
    write = sub.add_parser("set")
    write.add_argument("key", choices=WRITABLE_KEYS)
    write.add_argument("value")
    sync = sub.add_parser("sync-versions")
    sync.add_argument("foundry_manifest", type=Path)
    args = parser.parse_args()

    document = parse(args.manifest)

    if args.command == "read":
        key, value = read_pin(args.manifest, document)
        print(f"pin_key={key}")
        print(f"pin_value={value}")
    elif args.command == "sync-versions":
        wanted = sync_versions(args.manifest, document, parse(args.foundry_manifest))
        for name, (current, target) in sorted(wanted.items()):
            print(f'{name}: "{current}" -> "{target}"')
        if not wanted:
            print("already in step with starknet-foundry")
    else:
        count = write_pin(args.manifest, document, args.key, args.value)
        print(f"pinned {count} starknet-foundry crates at {args.key} = {args.value}")


if __name__ == "__main__":
    main()
