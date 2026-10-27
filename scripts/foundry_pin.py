#!/usr/bin/env python3
"""Read and bump the starknet-foundry pin in Cargo.toml.

Every starknet-foundry crate is pinned at the same point in upstream history,
either by `rev = "<sha>"` (check-starknet-foundry-bump.yml, tracking master) or
by `tag = "<tag>"` (bump-starknet-foundry-release.yml, following releases).

Moving the pin alone is not enough: we also depend directly on crates foundry
pins rigidly - cairo-lang-* at `=x.y.z`, blockifier and starknet_api at release
candidates - and Cargo will not unify those with anything else. So `bump` also
copies foundry's requirement onto every dependency both workspaces share where
either side is exact or a prerelease, and onto the whole cairo-lang-* family,
which must move in lockstep. Caret requirements are left alone, since Cargo
unifies them or builds both side by side, and our `=` pins stay exact.

Edits go through tomlkit (`pip install tomlkit==0.13.3`), which keeps comments
and formatting.

Usage:
    scripts/foundry_pin.py read      # pin_key=tag\\npin_value=v0.63.0
    scripts/foundry_pin.py bump tag v0.63.0 path/to/foundry/Cargo.toml
    scripts/foundry_pin.py bump rev <sha> path/to/foundry/Cargo.toml
"""

import argparse
from pathlib import Path

import tomlkit

# The ways a starknet-foundry crate is pinned here. Cargo itself rejects a
# dependency with more than one, so each crate carries exactly one.
LOCATOR_KEYS = ("tag", "rev")

# Crates released together from the Cairo compiler repo. They have to agree on
# one version, so all of them follow foundry, including ones it does not use.
CAIRO_FAMILY = "cairo-lang-"


def workspace_deps(parsed_manifest):
    """[workspace.dependencies] - both this workspace and starknet-foundry's are
    virtual roots, so every dependency either side declares is in there."""
    return parsed_manifest["workspace"]["dependencies"]


def find_starknet_foundry_deps(parsed_manifest):
    """{name: spec} for every dependency pinned to a starknet-foundry remote."""
    return {
        name: spec
        for name, spec in workspace_deps(parsed_manifest).items()
        if isinstance(spec, dict) and "starknet-foundry" in spec.get("git", "")
    }


def read_pin(parsed_manifest):
    """The single (key, value) every starknet-foundry dependency agrees on.

    Cargo would accept the crates at different points of the same repo, so the
    agreement is checked here - set_pin also relies on one shared locator key.
    """
    pins = set()
    for spec in find_starknet_foundry_deps(parsed_manifest).values():
        key = next(key for key in LOCATOR_KEYS if key in spec)
        pins.add((key, str(spec[key])))
    if len(pins) > 1:
        found = ", ".join(f'{k} = "{v}"' for k, v in pins)
        raise SystemExit(f"starknet-foundry crates disagree on the pin: {found}")
    return pins.pop()


def set_pin(parsed_manifest, key, value):
    """Points every starknet-foundry crate at `key = value`, in memory."""
    deps = find_starknet_foundry_deps(parsed_manifest)
    current_key, _ = read_pin(parsed_manifest)

    # tomlkit has no public way to rename a key in place: deleting and re-adding
    # moves it to the end of the table, and rebuilding the table drops its
    # spacing. Container._replace keeps both - it is private, which is why tomlkit
    # is pinned to an exact version in the workflows.
    for spec in deps.values():
        spec.value._replace(current_key, key, tomlkit.item(value))


def versioned_deps(parsed_manifest):
    """{name: version requirement} for every dependency that has one.

    Converted to plain str: tomlkit strings carry their formatting, such as a
    trailing comment, which must not follow a version from foundry's manifest
    into ours.
    """
    deps = {}
    for name, spec in workspace_deps(parsed_manifest).items():
        if isinstance(spec, str):
            deps[name] = str(spec)
        elif "version" in spec:
            deps[name] = str(spec["version"])
    return deps


def is_rigid(requirement):
    """An exact or prerelease requirement - one Cargo will not float to unify."""
    return requirement.lstrip().startswith("=") or "-" in requirement


def keep_exact(current, target):
    """Foundry's requirement, written back as exact where ours was exact."""
    target = target.strip()
    if not target.startswith("=") and current.lstrip().startswith("="):
        # A bare version is a caret requirement - drop an explicit `^` too.
        return "=" + target.lstrip("^")
    return target


def sync_versions(parsed_manifest, foundry_manifest):
    """Moves the dependencies that must follow foundry to its versions, in memory."""
    ours = workspace_deps(parsed_manifest)
    theirs = versioned_deps(foundry_manifest)
    # Foundry's cairo-lang-* crates all share one version - its own workspace
    # would not resolve otherwise - so any of them gives it.
    cairo = next(v for name, v in theirs.items() if name.startswith(CAIRO_FAMILY))

    for name, current in versioned_deps(parsed_manifest).items():
        if name in theirs:
            target = theirs[name]
            if not (is_rigid(current) or is_rigid(target)):
                continue
        elif name.startswith(CAIRO_FAMILY):
            target = cairo
        else:
            continue
        target = keep_exact(current, target)
        if isinstance(ours[name], dict):
            ours[name]["version"] = target
        else:
            # Assigning over a bare string keeps its trailing comment.
            ours[name] = target


def parse(path):
    try:
        return tomlkit.parse(path.read_text())
    except tomlkit.exceptions.ParseError as exc:
        raise SystemExit(f"{path}: {exc}") from exc


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--manifest",
        type=Path,
        default=Path(__file__).resolve().parent.parent / "Cargo.toml",
    )
    subcommands = parser.add_subparsers(dest="command", required=True)
    subcommands.add_parser("read")
    bump_cmd = subcommands.add_parser("bump")
    bump_cmd.add_argument("key", choices=LOCATOR_KEYS)
    bump_cmd.add_argument("value")
    bump_cmd.add_argument(
        "foundry_manifest",
        type=Path,
        help="starknet-foundry's Cargo.toml at the new pin",
    )
    args = parser.parse_args()

    parsed_manifest = parse(args.manifest)

    if args.command == "read":
        key, value = read_pin(parsed_manifest)
        print(f"pin_key={key}")
        print(f"pin_value={value}")
    else:
        set_pin(parsed_manifest, args.key, args.value)
        sync_versions(parsed_manifest, parse(args.foundry_manifest))
        args.manifest.write_text(tomlkit.dumps(parsed_manifest))


if __name__ == "__main__":
    main()
