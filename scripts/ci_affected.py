#!/usr/bin/env python3
"""Select PR test packages; unknown inputs fall back to the full workspace.

Uses only the Python standard library and cargo metadata --no-deps. Lockfile
edges from BOTH revisions cover removed/upgraded transitive dependencies.
Package names deliberately collapse multiple registry versions: over-testing
is preferable to overlooking a consumer. See docs/ci.md for the policy.
"""

import argparse
from collections import defaultdict
import json
import os
from pathlib import Path
import subprocess
import sys
import tomllib


def output(*args, cwd=None):
    return subprocess.check_output(args, cwd=cwd, text=True)


def closure(seeds, reverse):
    seen = set(seeds)
    pending = list(seen)
    while pending:
        for consumer in reverse.get(pending.pop(), ()):
            if consumer not in seen:
                seen.add(consumer)
                pending.append(consumer)
    return seen


def plan(packages=(), *, full=False, reason):
    packages = sorted(packages)
    return {
        "full": full,
        "run": full or bool(packages),
        "packages": packages,
        "dev_reload": full or "r2e-core" in packages,
        "compile_tests": full or "r2e-compile-tests" in packages,
        "reason": reason,
    }


def lock_packages(lock):
    if lock.get("version") not in (3, 4):
        raise ValueError("unsupported Cargo.lock format")
    entries = defaultdict(list)
    for package in lock["package"]:
        entries[package["name"]].append(json.dumps(package, sort_keys=True))
    return {name: sorted(values) for name, values in entries.items()}


def select(metadata, paths, read_base, read_head):
    root = Path(metadata["workspace_root"])
    members = set(metadata["workspace_members"])
    packages = [p for p in metadata["packages"] if p["id"] in members]
    names = {p["name"] for p in packages}
    owners = sorted(
        ((Path(p["manifest_path"]).parent.relative_to(root).as_posix(), p["name"])
         for p in packages),
        key=lambda pair: len(pair[0]), reverse=True,
    )
    reverse = defaultdict(set)
    for package in packages:
        # Includes renamed, optional, target, build and dev dependencies, even
        # when they are not enabled in the current default-feature resolution.
        for dep in package["dependencies"]:
            reverse[dep["name"]].add(package["name"])

    seeds = set()
    for path in paths:
        if path == "Cargo.lock":
            before = tomllib.loads(read_base(path))
            after = tomllib.loads(read_head(path))
            old, new = lock_packages(before), lock_packages(after)
            if {k: v for k, v in before.items() if k != "package"} != {
                k: v for k, v in after.items() if k != "package"
            }:
                return plan(full=True, reason="Cargo.lock format/metadata changed")
            seeds.update(n for n in old.keys() | new.keys() if old.get(n) != new.get(n))
            for lock in (before, after):
                for package in lock["package"]:
                    for dep in package.get("dependencies", []):
                        reverse[dep.split()[0]].add(package["name"])
        elif path == "Cargo.toml":
            before = tomllib.loads(read_base(path))
            after = tomllib.loads(read_head(path))
            old = before.get("workspace", {}).pop("dependencies", {})
            new = after.get("workspace", {}).pop("dependencies", {})
            if before != after:
                return plan(full=True, reason="workspace structure/settings changed")
            changed = {n for n in old.keys() | new.keys() if old.get(n) != new.get(n)}
            for package in packages:
                if any((d.get("rename") or d["name"]) in changed
                       for d in package["dependencies"]):
                    seeds.add(package["name"])
        elif path.startswith("llm/") or path == "llm.txt":
            seeds.update({"r2e-cli", "llm-doctests"} & names)
        elif path.startswith("docs/features/"):
            seeds.update({"r2e-cli"} & names)
        else:
            # Deleted or moved manifests can remove nodes from current metadata.
            if path.endswith("/Cargo.toml"):
                try:
                    read_head(path)
                except FileNotFoundError:
                    return plan(full=True, reason=f"removed manifest: {path}")
            owner = next((name for directory, name in owners
                          if path.startswith(directory + "/")), None)
            if owner:
                seeds.add(owner)
            # Only explicitly known prose-only locations may skip Rust tests.
            elif (path.startswith(("docs/book/", "docs/claude/", "docs/migration/", "plans/"))
                  and path.endswith(".md")) or path in {
                      "README.md", "CHANGELOG.md", "CLAUDE.md", "AGENTS.md", "llm-full.txt",
                  }:
                continue
            else:
                return plan(full=True, reason=f"global or unclassified input: {path}")

    affected = closure(seeds, reverse) & names
    return plan(affected, reason="changed crates and transitive consumers" if affected
                else "no Rust packages affected")


def git_plan(root, base):
    # Compare the base SHA to the actual tested checkout (the PR merge commit
    # in Actions). --no-renames exposes BOTH old and new paths for moves.
    paths = output("git", "diff", "--no-renames", "--name-only", "-z",
                   base, "HEAD", "--", cwd=root).rstrip("\0").split("\0")
    paths = [p for p in paths if p]
    metadata = json.loads(output("cargo", "metadata", "--no-deps", "--format-version", "1", cwd=root))
    return select(metadata, paths,
                  lambda p: output("git", "show", f"{base}:{p}", cwd=root),
                  lambda p: (root / p).read_text())


def cargo_command(selection, phase, toolchain=None, metadata=None):
    cargo = ["cargo"] + ([f"+{toolchain}"] if toolchain else [])
    if phase == "compile-tests":
        return cargo + ["test", "-p", "r2e-compile-tests"] if selection["compile_tests"] else []
    packages = selection["packages"]
    if phase == "test":
        packages = [p for p in packages if p != "r2e-compile-tests"]
    if selection["full"]:
        args = ["--workspace"]
        if phase == "test":
            args += ["--exclude", "r2e-compile-tests"]
    elif not packages:
        return []
    else:
        args = [arg for package in packages for arg in ("-p", package)]
        # Preserve each selected package's workspace-resolved features so its
        # feature-gated tests still run when an activating example is omitted.
        members = {p["id"]: p["name"] for p in metadata["packages"]
                   if p["id"] in metadata["workspace_members"]}
        features = sorted(f"{members[node['id']]}/{feature}"
                          for node in metadata["resolve"]["nodes"]
                          if members.get(node["id"]) in packages
                          for feature in node["features"])
        if features:
            args += ["--features", ",".join(features)]
    return cargo + (["test"] if phase == "test" else ["check", "--all-targets"]) + args


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    choose = sub.add_parser("plan")
    choose.add_argument("--base", help="base commit; omitted = full suite")
    choose.add_argument("--github-output", type=Path)
    run = sub.add_parser("run")
    run.add_argument("phase", choices=["test", "msrv", "compile-tests"])
    run.add_argument("--toolchain")
    args = parser.parse_args()
    if args.command == "plan":
        root = Path(output("git", "rev-parse", "--show-toplevel").strip())
        try:
            selection = git_plan(root, args.base) if args.base else plan(full=True, reason="full CI run")
        except (OSError, ValueError, KeyError, TypeError, subprocess.CalledProcessError) as exc:
            print(f"Selection unavailable; running full suite: {exc}", file=sys.stderr)
            selection = plan(full=True, reason="diff or dependency analysis failed")
        encoded = json.dumps(selection, separators=(",", ":"))
        print(json.dumps(selection, indent=2))
        if args.github_output:
            with args.github_output.open("a") as stream:
                stream.write(f"plan={encoded}\nrun={str(selection['run']).lower()}\n")
        if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
            with open(summary, "a") as stream:
                stream.write("### Rust test selection\n\n```json\n" + json.dumps(selection, indent=2) + "\n```\n")
    else:
        selection = json.loads(os.environ["CI_PLAN"])
        metadata = None
        if not selection["full"] and selection["run"] and args.phase != "compile-tests":
            # Resolve on the job's toolchain; --locked refuses accidental lock
            # updates. A failure here fails the job instead of dropping tests.
            cargo = ["cargo"] + ([f"+{args.toolchain}"] if args.toolchain else [])
            metadata = json.loads(output(*cargo, "metadata", "--locked", "--format-version", "1"))
        command = cargo_command(selection, args.phase, args.toolchain, metadata)
        if command:
            print("Running:", " ".join(command), flush=True)
            subprocess.run(command, check=True)
        else:
            print("No affected packages for this phase.")


if __name__ == "__main__":
    main()
