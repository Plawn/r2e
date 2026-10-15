#!/usr/bin/env python3
"""versions.py — the per-crate versioning model of this workspace, in one place.

Every publishable crate carries its own `version` (there is no
`[workspace.package] version`), the `[workspace.dependencies]` pins mirror those
versions exactly, and a release publishes only the crates whose version changed.
`scripts/bump-version.sh`, `scripts/check-versions.sh`,
`scripts/publish-crates.sh`, `scripts/check-release-drift.sh` and
`.github/workflows/release.yml` all go through this file rather than
re-deriving the rules.

Rules (also in CLAUDE.md § Project Status):

  * pre-1.0, a breaking change bumps the crate's MINOR, anything else its PATCH;
  * a breaking bump PROPAGATES: every publishable crate that depends on the
    bumped one (normal or build dependency, transitively) takes a breaking bump
    too, because it re-exports that crate's types — the facade `r2e` always
    ends up in that set. A patch bump propagates nothing (caret requirements);
  * `r2e-cli` FOLLOWS `r2e` version for version (it scaffolds against, and
    embeds the agent reference of, that facade version);
  * a feature PR never bumps; the `release: …` PR made by bump-version.sh does;
  * a release leaves NOTHING BEHIND: every releasable crate whose directory
    moved since the tag that released it (CHANGELOG.md and the manifest's
    version line aside) is in the plan. `plan`/`bump` refuse otherwise, and
    `check` fails on it while a release is in flight (#1092);
  * `r2e-macros` is the one crate whose breaking bump is cut with
    `--no-propagate`: it breaks its codegen contract with r2e-core, not the
    macro syntax, and its direct consumers re-pin it with a patch bump.

Subcommands:
  check                           verify every invariant (CI + bump-version.sh)
  plan   [--no-propagate] SPEC…   print the bump plan, change nothing
  bump   [--no-propagate] SPEC…   apply it: manifests, pins, changelogs, CLI pin
  decide [--base REF]             what this commit releases, as JSON (release.yml)
  uploaded                        crates already on crates.io at their version
  held-back                       crates kept off crates.io by policy
  left-behind                     released crates that moved since their tag
  notes CRATE [VERSION]           a crate's changelog section (release notes)
  version CRATE                   print a crate's version

  SPEC = <crate> <patch|minor|major|breaking|X.Y.Z>
"""

import datetime as _dt
import json
import os
import re
import subprocess
import sys
import urllib.error
import urllib.request
from dataclasses import dataclass, field

# Crates that stay off crates.io for now; the reason is in publish-crates.sh.
HELD_BACK = {"r2e-cli"}

# follower -> leader: the follower is bumped to the leader's version whenever
# the leader moves, and may not be bumped on its own.
FOLLOWS = {"r2e-cli": "r2e"}

# `r2e add` writes this requirement for every R2E dependency it inserts.
CLI_DEP_VERSION_FILE = "r2e-cli/src/commands/add.rs"
CLI_DEP_VERSION_RE = re.compile(r'^(pub const R2E_DEP_VERSION: &str = ")([^"]+)(";)$', re.M)

LLM_FULL = "llm-full.txt"


def die(msg):
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def sh(*args, **kw):
    return subprocess.check_output(args, text=True, **kw)


ROOT = sh("git", "rev-parse", "--show-toplevel").strip()


# ---------------------------------------------------------------------------
# Versions
# ---------------------------------------------------------------------------

Version = tuple


def parse_version(s):
    m = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)", s)
    if not m:
        die(f"not a plain X.Y.Z version: {s!r}")
    return tuple(int(x) for x in m.groups())


def fmt(v):
    return ".".join(str(x) for x in v)


def breaking_bump(v):
    """The bump kind that signals a breaking change at this version."""
    return "major" if v[0] >= 1 else "minor"


def next_version(cur, kind):
    major, minor, patch = cur
    if kind == "patch":
        return (major, minor, patch + 1)
    if kind == "minor":
        return (major, minor + 1, 0)
    if kind == "major":
        return (major + 1, 0, 0)
    if kind == "breaking":
        return next_version(cur, breaking_bump(cur))
    new = parse_version(kind)
    if new <= cur:
        die(f"{fmt(new)} is not above the current {fmt(cur)}")
    return new


def is_breaking(old, new):
    """Does old -> new cross a compatibility boundary (semver, pre-1.0 aware)?"""
    if old[0] == 0:
        return new[:2] != old[:2]
    return new[0] != old[0]


# ---------------------------------------------------------------------------
# Workspace model (cargo metadata, no resolution)
# ---------------------------------------------------------------------------


@dataclass
class Crate:
    name: str
    version: Version
    dir: str  # relative to ROOT
    manifest: str  # absolute
    publish: bool
    deps: dict = field(default_factory=dict)  # workspace crate -> req (normal/build)
    dev_deps: dict = field(default_factory=dict)  # workspace crate -> req (dev)


def workspace():
    meta = json.loads(sh("cargo", "metadata", "--no-deps", "--format-version", "1", cwd=ROOT))
    names = {p["name"] for p in meta["packages"]}
    ws = {}
    for p in meta["packages"]:
        c = Crate(
            name=p["name"],
            version=parse_version(p["version"]),
            dir=os.path.relpath(os.path.dirname(p["manifest_path"]), ROOT),
            manifest=p["manifest_path"],
            publish=p.get("publish") != [],
        )
        for d in p["dependencies"]:
            if d["name"] not in names:
                continue
            (c.dev_deps if d["kind"] == "dev" else c.deps)[d["name"]] = d["req"]
        ws[c.name] = c
    return ws


def publishable(ws):
    return [c for c in ws.values() if c.publish]


def releasable(ws):
    return [c for c in publishable(ws) if c.name not in HELD_BACK]


def reverse_deps(ws):
    rdeps = {n: set() for n in ws}
    for c in publishable(ws):
        for d in c.deps:
            rdeps[d].add(c.name)
    return rdeps


# ---------------------------------------------------------------------------
# TOML slices (the manifests are simple enough for line regexes; no tomllib
# dependency so this runs on any python3)
# ---------------------------------------------------------------------------


def section(text, header):
    """(start, end) of the body of `[header]` in text, or None."""
    m = re.search(r"(?m)^\[" + re.escape(header) + r"\]\s*$", text)
    if not m:
        return None
    start = m.end()
    nxt = re.search(r"(?m)^\[", text[start:])
    end = start + nxt.start() if nxt else len(text)
    return start, end


def package_version_literal(manifest_text):
    """The `[package] version = "…"` literal, or None when inherited/absent."""
    span = section(manifest_text, "package")
    if not span:
        return None
    body = manifest_text[span[0] : span[1]]
    m = re.search(r'(?m)^version\s*=\s*"([^"]+)"\s*$', body)
    return m.group(1) if m else None


def workspace_version_literal(root_text):
    span = section(root_text, "workspace.package")
    if not span:
        return None
    m = re.search(r'(?m)^version\s*=\s*"([^"]+)"\s*$', root_text[span[0] : span[1]])
    return m.group(1) if m else None


def git_show(ref, path):
    try:
        return sh("git", "show", f"{ref}:{path}", cwd=ROOT, stderr=subprocess.DEVNULL)
    except subprocess.CalledProcessError:
        return None


def version_at(ref, crate):
    """The crate's version at `ref`, resolving the old `version.workspace = true`."""
    text = git_show(ref, f"{crate.dir}/Cargo.toml")
    if text is None:
        return None
    lit = package_version_literal(text)
    if lit:
        return lit
    root = git_show(ref, "Cargo.toml")
    return workspace_version_literal(root) if root else None


PIN_RE_TEMPLATE = r'(?m)^({name}\s*=\s*\{{[^}}\n]*\bversion\s*=\s*")([^"]+)(")'


def pin_regex(name):
    return re.compile(PIN_RE_TEMPLATE.format(name=re.escape(name)))


def read(path):
    with open(os.path.join(ROOT, path), encoding="utf-8") as f:
        return f.read()


def write(path, text):
    with open(os.path.join(ROOT, path), "w", encoding="utf-8") as f:
        f.write(text)


# ---------------------------------------------------------------------------
# Tags and crates.io
# ---------------------------------------------------------------------------


def local_tags():
    return set(sh("git", "tag", "-l", cwd=ROOT).split())


def remote_tags():
    try:
        out = sh("git", "ls-remote", "--tags", "--refs", "origin", cwd=ROOT, stderr=subprocess.DEVNULL)
    except subprocess.CalledProcessError:
        print("warning: cannot list origin's tags (offline?) — checked local tags only", file=sys.stderr)
        return set()
    return {line.split("refs/tags/", 1)[1] for line in out.splitlines() if "refs/tags/" in line}


def tag_of(name, version):
    return f"{name}-v{fmt(version)}"


def release_tag(tags, name, version):
    """The tag that released (name, version): its own, else the legacy lockstep
    `vX.Y.Z` that covered every crate up to 0.6.0; None when unreleased."""
    own = tag_of(name, version)
    if own in tags:
        return own
    legacy = f"v{fmt(version)}"
    return legacy if legacy in tags else None


def released(tags, name, version):
    return release_tag(tags, name, version) is not None


def on_crates_io(name, version):
    req = urllib.request.Request(
        f"https://crates.io/api/v1/crates/{name}",
        headers={"User-Agent": f"r2e-versions-script ({name} {fmt(version)})"},
    )
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            data = json.load(resp)
    except urllib.error.HTTPError as exc:
        if exc.code == 404:  # name not taken yet
            return False
        raise
    return any(v["num"] == fmt(version) for v in data.get("versions", []))


# ---------------------------------------------------------------------------
# Left behind: released crates whose source moved since their tag
# ---------------------------------------------------------------------------
#
# A published (name, version) is immutable, so a crate that changed since the
# tag that released it is stale on crates.io until it is bumped. A feature PR
# may leave crates in that state — that is what "a feature PR never bumps"
# means — but a release may not: the release that cut r2e-core 0.7.0 left
# r2e-macros at 0.6.0 although its codegen had moved with core, and every
# registry build of r2e 0.7/0.8 failed while the workspace, built from source,
# stayed green (#1092). Hence the rule `plan`/`bump` enforce, and `check`
# enforces while a release is in flight: NOTHING IS LEFT BEHIND — every crate
# that moved since its tag is in the plan.

# Files a feature PR edits without changing what the crate ships.
UNSHIPPED_FILES = {"CHANGELOG.md"}

# The one manifest line a release itself rewrites (and the `version.workspace`
# spelling it replaced when the lockstep ended).
VERSION_LINE_RE = re.compile(r"(?m)^version(\.workspace)?\s*=.*$")


def nested_members(ws, crate):
    """Directories of the other workspace members that live under `crate`'s
    directory (`r2e-openfga/model`): cargo excludes them from its package."""
    prefix = crate.dir + os.sep
    return [c.dir for c in ws.values() if c is not crate and c.dir.startswith(prefix)]


def moved_since(ws, crate, tag):
    """Paths under the crate (relative to it) that differ between `tag` and the
    working tree, ignoring nested members, UNSHIPPED_FILES and a manifest whose
    only change is its version line."""
    out = sh("git", "diff", "--name-only", tag, "--", crate.dir, cwd=ROOT)
    nested = nested_members(ws, crate)
    moved = []
    for path in out.split():
        if any(path.startswith(d + os.sep) for d in nested):
            continue
        rel = os.path.relpath(path, crate.dir)
        if rel in UNSHIPPED_FILES:
            continue
        if rel == "Cargo.toml":
            before = VERSION_LINE_RE.sub("", git_show(tag, path) or "")
            now = read(path) if os.path.isfile(os.path.join(ROOT, path)) else ""
            if before == VERSION_LINE_RE.sub("", now):
                continue
        moved.append(rel)
    return moved


def left_behind(ws, tags):
    """[(crate, tag, moved)] for every releasable crate that is released at its
    current version and has moved since."""
    out = []
    for c in sorted(releasable(ws), key=lambda c: c.name):
        tag = release_tag(tags, c.name, c.version)
        if tag is not None:
            moved = moved_since(ws, c, tag)
            if moved:
                out.append((c, tag, moved))
    return out


def in_flight(ws, tags):
    """Releasable crates whose current version has no tag yet: a release is
    being cut, or one did not finish."""
    return [c for c in releasable(ws) if release_tag(tags, c.name, c.version) is None]


def describe_moved(moved, limit=4):
    more = f", … ({len(moved)} files)" if len(moved) > limit else ""
    return ", ".join(moved[:limit]) + more


# ---------------------------------------------------------------------------
# Changelogs
# ---------------------------------------------------------------------------

UNRELEASED = "## [Unreleased]"


def changelog_path(crate):
    return os.path.join(crate.dir, "CHANGELOG.md")


def changelog_section(text, heading_prefix):
    """(start, end) of the body under the `## […]` heading starting with
    heading_prefix (up to the next `## ` heading), or None."""
    m = re.search(r"(?m)^" + re.escape(heading_prefix) + r".*$", text)
    if not m:
        return None
    start = m.end()
    nxt = re.search(r"(?m)^## ", text[start:])
    end = start + nxt.start() if nxt else len(text)
    return start, end


def section_is_empty(text, heading_prefix):
    span = changelog_section(text, heading_prefix)
    return span is None or not text[span[0] : span[1]].strip()


# ---------------------------------------------------------------------------
# check
# ---------------------------------------------------------------------------


def cmd_check(_args):
    ws = workspace()
    errors = []

    root = read("Cargo.toml")
    if workspace_version_literal(root) is not None:
        errors.append("Cargo.toml: [workspace.package] must not carry a `version` — crates are versioned independently")

    for c in ws.values():
        text = read(os.path.join(c.dir, "Cargo.toml"))
        lit = package_version_literal(text)
        if lit is None:
            errors.append(f"{c.dir}/Cargo.toml: [package] must declare its own `version = \"…\"`")
        if c.publish:
            if c.version == (0, 0, 0):
                errors.append(f"{c.name}: a publishable crate cannot be 0.0.0")
            if not os.path.isfile(os.path.join(ROOT, changelog_path(c))):
                errors.append(f"{changelog_path(c)}: missing (every publishable crate keeps its own changelog)")
            elif UNRELEASED not in read(changelog_path(c)):
                errors.append(f"{changelog_path(c)}: no `{UNRELEASED}` section")
            for dep, req in c.deps.items():
                d = ws[dep]
                if not d.publish:
                    errors.append(f"{c.name} -> {dep}: a published crate depends on an unpublished one")
                elif req != f"^{fmt(d.version)}":
                    errors.append(
                        f"{c.name} -> {dep}: requirement {req!r} but {dep} is at {fmt(d.version)} "
                        f"(pins in [workspace.dependencies] must equal the crate's version)"
                    )
        elif c.version != (0, 0, 0):
            errors.append(f"{c.name}: `publish = false` crates are versioned 0.0.0 (found {fmt(c.version)})")

    # Pins: every entry of [workspace.dependencies] that names a workspace crate.
    span = section(root, "workspace.dependencies")
    if not span:
        errors.append("Cargo.toml: no [workspace.dependencies] section")
    else:
        for m in re.finditer(r'(?m)^([A-Za-z0-9_-]+)\s*=\s*\{([^}\n]*)\}', root[span[0] : span[1]]):
            name, body = m.group(1), m.group(2)
            if name not in ws:
                continue
            pv = re.search(r'\bversion\s*=\s*"([^"]+)"', body)
            pp = re.search(r'\bpath\s*=\s*"([^"]+)"', body)
            if not pv:
                errors.append(f"Cargo.toml: pin `{name}` has no version (it would not publish)")
            elif pv.group(1) != fmt(ws[name].version):
                errors.append(f"Cargo.toml: pin `{name}` = {pv.group(1)} but the crate is at {fmt(ws[name].version)}")
            if pp and os.path.normpath(pp.group(1)) != os.path.normpath(ws[name].dir):
                errors.append(f"Cargo.toml: pin `{name}` points at {pp.group(1)}, the crate lives in {ws[name].dir}")

    for follower, leader in FOLLOWS.items():
        if ws[follower].version != ws[leader].version:
            errors.append(
                f"{follower} is at {fmt(ws[follower].version)} but follows {leader} "
                f"({fmt(ws[leader].version)}) — bump {leader}, never {follower} alone"
            )

    facade = ws["r2e"].version
    m = CLI_DEP_VERSION_RE.search(read(CLI_DEP_VERSION_FILE))
    want = f"{facade[0]}.{facade[1]}"
    if not m:
        errors.append(f"{CLI_DEP_VERSION_FILE}: cannot find R2E_DEP_VERSION")
    elif m.group(2) != want:
        errors.append(f"{CLI_DEP_VERSION_FILE}: R2E_DEP_VERSION = {m.group(2)!r}, r2e is at {fmt(facade)} (want {want!r})")

    first = read(LLM_FULL).splitlines()[0] if os.path.isfile(os.path.join(ROOT, LLM_FULL)) else ""
    if f"R2E v{fmt(facade)} " not in first:
        errors.append(f"{LLM_FULL}: not stamped with r2e {fmt(facade)} — run scripts/check-llm-docs.sh --update")

    # Nothing left behind — binding only while a release is in flight (some
    # crate's current version is untagged): a feature PR leaves crates stale
    # by design, a release PR may not. Needs the tags: CI checks out with
    # fetch-tags, a shallow clone is refused rather than read as "all stale".
    tags = local_tags()
    stale = []
    if not any(released(tags, c.name, c.version) for c in releasable(ws)):
        errors.append("no release tag found for any crate — shallow clone? `git fetch --tags` before checking")
    else:
        stale = left_behind(ws, tags)
        flight = in_flight(ws, tags)
        if stale and flight:
            cutting = ", ".join(f"{c.name} {fmt(c.version)}" for c in flight)
            for c, tag, moved in stale:
                errors.append(
                    f"{c.name} {fmt(c.version)} moved since {tag} ({describe_moved(moved)}) but the release in "
                    f"flight ({cutting}) does not bump it — crates.io would keep serving the old {fmt(c.version)}. "
                    f"Add `{c.name} patch` to the plan (or `git fetch --tags` if that release already completed)"
                )

    if errors:
        for e in errors:
            print(f"  {e}", file=sys.stderr)
        die(f"{len(errors)} versioning problem(s)")
    n = len(publishable(ws))
    print(f"ok — {n} publishable crates, pins and changelogs consistent, r2e at {fmt(facade)}")
    for c, tag, moved in stale:
        print(f"  note: {c.name} {fmt(c.version)} moved since {tag} ({describe_moved(moved)}) — the next release must bump it")


# ---------------------------------------------------------------------------
# plan / bump
# ---------------------------------------------------------------------------

KINDS = ("patch", "minor", "major", "breaking")


@dataclass
class Bump:
    new: Version
    reason: str
    requested: bool
    via: list = field(default_factory=list)  # direct deps whose breaking bump reached us


def parse_specs(ws, args):
    propagate = True
    rest = []
    for a in args:
        if a == "--no-propagate":
            propagate = False
        elif a.startswith("-"):
            die(f"unknown flag {a}")
        else:
            rest.append(a)
    if not rest or len(rest) % 2:
        die("usage: <crate> <patch|minor|major|breaking|X.Y.Z> [<crate> <bump> …]")
    specs = []
    for name, kind in zip(rest[::2], rest[1::2]):
        if name not in ws:
            die(f"no workspace crate named {name!r}")
        if not ws[name].publish:
            die(f"{name} is `publish = false` — it has no version to bump")
        if name in FOLLOWS:
            die(f"{name} follows {FOLLOWS[name]} — bump {FOLLOWS[name]} instead")
        if kind not in KINDS:
            parse_version(kind)
        specs.append((name, kind))
    return propagate, specs


def make_plan(ws, specs, propagate):
    bumps = {}
    for name, kind in specs:
        bumps[name] = Bump(next_version(ws[name].version, kind), "requested", True)

    if propagate:
        rdeps = reverse_deps(ws)
        queue = [n for n, b in bumps.items() if is_breaking(ws[n].version, b.new)]
        visited = set(queue)
        while queue:
            n = queue.pop(0)
            for dep in sorted(rdeps[n]):
                c = ws[dep]
                if not c.publish:
                    continue
                floor = next_version(c.version, "breaking")
                b = bumps.get(dep)
                if b is None:
                    b = bumps[dep] = Bump(floor, f"depends on {n}", False)
                elif b.new < floor:
                    b.new = floor
                    b.reason = f"requested, raised: depends on {n}"
                b.via.append(n)
                if dep not in visited:
                    visited.add(dep)
                    queue.append(dep)

    for follower, leader in FOLLOWS.items():
        if leader in bumps:
            bumps[follower] = Bump(bumps[leader].new, f"follows {leader}", False)

    # Deterministic, bottom-up-ish: by name is enough for display.
    return dict(sorted(bumps.items()))


def print_plan(ws, bumps):
    width = max(len(n) for n in bumps)
    print("Bump plan:", file=sys.stderr)
    for name, b in bumps.items():
        kind = "breaking" if is_breaking(ws[name].version, b.new) else "patch"
        print(f"  {name:<{width}}  {fmt(ws[name].version):>8} -> {fmt(b.new):<8} {kind:<9} {b.reason}", file=sys.stderr)
    untouched = len(publishable(ws)) - len(bumps)
    print(f"  ({untouched} publishable crate(s) untouched)", file=sys.stderr)


def validate_plan(ws, bumps, tags):
    errors = []
    root = read("Cargo.toml")
    for name, b in bumps.items():
        c = ws[name]
        tag = tag_of(name, b.new)
        if released(tags, name, b.new):
            errors.append(f"{name} {fmt(b.new)} is already tagged ({tag} or v{fmt(b.new)})")
        if on_crates_io(name, b.new):
            errors.append(f"{name} {fmt(b.new)} is already on crates.io")
        m = pin_regex(name).search(root)
        if m and m.group(2) != fmt(c.version):
            errors.append(f"Cargo.toml: pin `{name}` = {m.group(2)} but the crate is at {fmt(c.version)} — fix it first")
        if package_version_literal(read(os.path.join(c.dir, "Cargo.toml"))) != fmt(c.version):
            errors.append(f"{c.dir}/Cargo.toml: cannot find `version = \"{fmt(c.version)}\"` under [package]")
        text = read(changelog_path(c))
        if UNRELEASED not in text:
            errors.append(f"{changelog_path(c)}: no `{UNRELEASED}` section")
        elif b.requested and section_is_empty(text, UNRELEASED):
            errors.append(f"{changelog_path(c)}: nothing under `{UNRELEASED}` — write the entries before cutting {fmt(b.new)}")
    errors += left_behind_by(ws, bumps, tags)
    if errors:
        for e in errors:
            print(f"  {e}", file=sys.stderr)
        die("the plan cannot be applied")


def left_behind_by(ws, bumps, tags):
    """One message per crate the plan leaves behind (moved since its tag, not bumped)."""
    return [
        f"{c.name} {fmt(c.version)} moved since {tag} ({describe_moved(moved)}) and is not in the plan — "
        f"nothing is left behind at a release: add `{c.name} patch` (or `breaking`)"
        for c, tag, moved in left_behind(ws, tags)
        if c.name not in bumps
    ]


REPO_URL = "https://github.com/Plawn/r2e"


def changelog_link(to, version):
    """Absolute link to a crate's CHANGELOG at its release tag: the entry is
    read both in the repo and on the GitHub release page, where a relative
    path would not resolve."""
    return f"{REPO_URL}/blob/{tag_of(to.name, version)}/{to.dir}/CHANGELOG.md"


def release_changelog(ws, bumps, name, today):
    c, b = ws[name], bumps[name]
    path = changelog_path(c)
    text = read(path)
    heading = f"## [{fmt(b.new)}] - {today}"
    text = text.replace(UNRELEASED + "\n", f"{UNRELEASED}\n\n{heading}\n", 1)

    extra = []
    if name in FOLLOWS:
        leader = FOLLOWS[name]
        extra.append(
            f"- Tracks `{leader}` {fmt(bumps[leader].new)}: `r2e new`, `r2e add` and "
            f"`r2e docs --llm` target that facade version."
        )
    for dep in b.via:
        extra.append(
            f"- Built against `{dep}` {fmt(bumps[dep].new)} — its breaking changes surface through "
            f"this crate's re-exports (see [`{dep}/CHANGELOG.md`]({changelog_link(ws[dep], bumps[dep].new)}))."
        )
    if extra:
        span = changelog_section(text, f"## [{fmt(b.new)}]")
        body = text[span[0] : span[1]].rstrip("\n")
        block = body + "\n\n### Changed\n\n" + "\n".join(extra) + "\n\n"
        text = text[: span[0]] + block + text[span[1] :]
    write(path, text)


def apply_plan(ws, bumps):
    today = _dt.date.today().isoformat()
    root = read("Cargo.toml")
    for name, b in bumps.items():
        c = ws[name]
        mpath = os.path.join(c.dir, "Cargo.toml")
        text = read(mpath)
        span = section(text, "package")
        body, n = re.subn(
            r'(?m)^version\s*=\s*"' + re.escape(fmt(c.version)) + r'"\s*$',
            f'version = "{fmt(b.new)}"',
            text[span[0] : span[1]],
            count=1,
        )
        assert n == 1, name
        write(mpath, text[: span[0]] + body + text[span[1] :])
        root, n = pin_regex(name).subn(lambda m, v=fmt(b.new): m.group(1) + v + m.group(3), root, count=1)
        release_changelog(ws, bumps, name, today)
    write("Cargo.toml", root)

    if "r2e" in bumps:
        facade = bumps["r2e"].new
        text, n = CLI_DEP_VERSION_RE.subn(
            lambda m: m.group(1) + f"{facade[0]}.{facade[1]}" + m.group(3), read(CLI_DEP_VERSION_FILE), count=1
        )
        assert n == 1, "R2E_DEP_VERSION"
        write(CLI_DEP_VERSION_FILE, text)


def title_of(bumps):
    return ", ".join(f"{n} {fmt(b.new)}" for n, b in bumps.items())


def cmd_plan(args):
    ws = workspace()
    propagate, specs = parse_specs(ws, args)
    bumps = make_plan(ws, specs, propagate)
    print_plan(ws, bumps)
    for e in left_behind_by(ws, bumps, local_tags()):
        print(f"warning: {e} — `bump` will refuse this plan", file=sys.stderr)
    print(title_of(bumps))


def cmd_bump(args):
    ws = workspace()
    propagate, specs = parse_specs(ws, args)
    bumps = make_plan(ws, specs, propagate)
    print_plan(ws, bumps)
    validate_plan(ws, bumps, local_tags() | remote_tags())
    apply_plan(ws, bumps)
    print(title_of(bumps))


# ---------------------------------------------------------------------------
# decide / uploaded / notes / misc
# ---------------------------------------------------------------------------


def cmd_decide(args):
    base = "HEAD^1"
    if args[:1] == ["--base"]:
        base = args[1]
    elif args:
        die("usage: decide [--base REF]")
    ws = workspace()
    tags = local_tags()
    release, errors = [], []
    for c in sorted(releasable(ws), key=lambda c: c.name):
        if released(tags, c.name, c.version):
            continue
        prev = version_at(base, c)
        if prev != fmt(c.version):
            release.append({"name": c.name, "version": fmt(c.version), "dir": c.dir, "tag": tag_of(c.name, c.version), "previous": prev})
        else:
            errors.append(
                f"{c.name} {fmt(c.version)} is not tagged, but this commit did not change it: that release "
                f"is incomplete. Re-run the Release run of the commit that bumped it (it resumes), or bump again."
            )
    print(json.dumps({"release": release, "errors": errors}, indent=2))
    if errors:
        sys.exit(1)


def cmd_uploaded(_args):
    ws = workspace()
    for c in sorted(releasable(ws), key=lambda c: c.name):
        if on_crates_io(c.name, c.version):
            print(f"{c.name}\t{fmt(c.version)}\t{c.dir}")


def cmd_held_back(_args):
    for n in sorted(HELD_BACK):
        print(n)


def cmd_left_behind(_args):
    ws = workspace()
    for c, tag, moved in left_behind(ws, local_tags()):
        print(f"{c.name}\t{fmt(c.version)}\t{tag}\t{' '.join(moved)}")


def cmd_notes(args):
    if not args or len(args) > 2:
        die("usage: notes CRATE [VERSION]")
    ws = workspace()
    c = ws.get(args[0]) or die(f"no workspace crate named {args[0]!r}")
    version = args[1] if len(args) == 2 else fmt(c.version)
    text = read(changelog_path(c))
    span = changelog_section(text, f"## [{version}]")
    if span is None or not text[span[0] : span[1]].strip():
        die(f"{changelog_path(c)}: no non-empty `## [{version}]` section")
    print(text[span[0] : span[1]].strip("\n"))


def cmd_version(args):
    ws = workspace()
    c = ws.get(args[0]) if args else None
    if not c:
        die("usage: version CRATE")
    print(fmt(c.version))


COMMANDS = {
    "check": cmd_check,
    "plan": cmd_plan,
    "bump": cmd_bump,
    "decide": cmd_decide,
    "uploaded": cmd_uploaded,
    "held-back": cmd_held_back,
    "left-behind": cmd_left_behind,
    "notes": cmd_notes,
    "version": cmd_version,
}


def main(argv):
    if len(argv) < 2 or argv[1] not in COMMANDS:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    COMMANDS[argv[1]](argv[2:])


if __name__ == "__main__":
    main(sys.argv)
