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
FOUNDRY_LOCATOR_KEYS = ("tag", "rev")

# Crates released together from the Cairo compiler repo. They have to agree on
# one version, so all of them follow foundry, including ones it does not use.
CAIRO_FAMILY = "cairo-lang-"


def workspace_deps(parsed_manifest):
    """[workspace.dependencies] - both this workspace and starknet-foundry's are
    virtual roots and every dependency either side declares is in there."""
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
    agreement is checked here.
    """
    pins = set()
    for name, spec in find_starknet_foundry_deps(parsed_manifest).items():
        key = next((key for key in FOUNDRY_LOCATOR_KEYS if key in spec), None)
        if key is None:
            raise SystemExit(
                f"{name} is a starknet-foundry dependency with no "
                f"{' or '.join(FOUNDRY_LOCATOR_KEYS)}"
            )
        pins.add((key, str(spec[key])))
    if not pins:
        raise SystemExit("no starknet-foundry dependencies found")
    if len(pins) > 1:
        found = ", ".join(f'{k} = "{v}"' for k, v in pins)
        raise SystemExit(f"starknet-foundry crates disagree on the pin: {found}")
    return pins.pop()


def set_pin(parsed_manifest, key, value):
    """Points every starknet-foundry crate at `key = value`, in memory."""
    deps = workspace_deps(parsed_manifest)
    for name, spec in find_starknet_foundry_deps(parsed_manifest).items():
        set_source(deps, name, {"git": str(spec["git"]), key: value})


# The keys that say where a dependency comes from: a crates.io version, or a git
# repo and a point in it. Everything else on a dependency (features,
# default-features, ...) is ours and is left alone.
SOURCE_KEYS = ("version", "git", "branch", "tag", "rev")


def source_of(spec):
    """The source keys of a dependency spec, e.g. {"version": "=2.19.5"}.

    Converted to plain str: tomlkit strings carry their formatting, such as a
    trailing comment, which must not follow a value from foundry's manifest
    into ours. Empty for path dependencies.
    """
    if isinstance(spec, str):
        return {"version": str(spec)}
    return {key: str(spec[key]) for key in SOURCE_KEYS if key in spec}


def is_pinned(source):
    """A source Cargo will not unify with another: a git, exact or prerelease one."""
    source_requirement = source.get("version", "")
    is_exact = source_requirement.lstrip().startswith("=")
    is_prerelease = "-" in source_requirement
    is_git = "git" in source
    return is_exact or is_prerelease or is_git


def keep_exact(current, target):
    """Foundry's requirement, written back as exact where ours was exact."""
    target = target.strip()
    if not target.startswith("=") and current.lstrip().startswith("="):
        # A bare version is a caret requirement - drop an explicit `^` too.
        return "=" + target.lstrip("^")
    return target


def set_source(deps, name, source):
    """Replaces the source keys of `deps[name]`, keeping the rest of its spec.

    The entry is rebuilt as one inline table - `{ k = v, k = v }`, as everywhere
    in our manifest - with the source first, as our entries all start with it,
    and our other keys (features, default-features, ...) rendered exactly as
    they were. Editing keys in place instead leaves tomlkit's separators behind.
    An entry that ends up as just a version is written as a bare string.
    Assigning over the old value keeps its trailing comment.
    """
    spec = deps[name]
    non_source_keys = [
        f"{key} = {spec.value.item(key).as_string()}"
        for key in (spec if isinstance(spec, dict) else ())
        if key not in SOURCE_KEYS
    ]
    if not non_source_keys and "git" not in source:
        deps[name] = source["version"]
        return
    pairs = [f"{key} = {tomlkit.item(v).as_string()}" for key, v in source.items()]
    deps[name] = tomlkit.parse(f"x = {{ {', '.join(pairs + non_source_keys)} }}")["x"]


def sync_versions(parsed_manifest, foundry_manifest):
    """Moves the dependencies that must follow foundry to its sources, in memory.

    Foundry has pinned blockifier and starknet_api both by crates.io version and
    by git branch, so a source is copied whole - a switch in either direction
    carries over.
    """
    ours = workspace_deps(parsed_manifest)
    theirs = {
        name: source_of(spec) for name, spec in workspace_deps(foundry_manifest).items()
    }
    # Foundry's cairo-lang-* crates all share one version - its own workspace
    # would not resolve otherwise - so any of them gives it.
    cairo = next(
        (
            source["version"]
            for name, source in theirs.items()
            if name.startswith(CAIRO_FAMILY) and "version" in source
        ),
        None,
    )
    if cairo is None:
        raise SystemExit("starknet-foundry declares no cairo-lang-* version")

    for name, spec in ours.items():
        current = source_of(spec)
        if name.startswith(CAIRO_FAMILY):
            # Checked first: the family moves together even where foundry
            # declares a member differently (or not at all).
            target = {"version": cairo}
        elif name in theirs:
            target = theirs[name]
            # A path dependency is a crate of foundry's own workspace - the
            # starknet-foundry pin covers those.
            is_path_dep = not target
            if is_path_dep or not (is_pinned(current) or is_pinned(target)):
                continue
        else:
            continue
        if "version" in current and "version" in target:
            target = {"version": keep_exact(current["version"], target["version"])}
        set_source(ours, name, target)


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
    bump_cmd.add_argument("key", choices=FOUNDRY_LOCATOR_KEYS)
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
