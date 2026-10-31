"""Regression tests for scripts/versions.py: the "new crate" rule.

A crate never tagged `<crate>-v*` lands through a feature PR with only
`## [Unreleased]` in its changelog. It must not count as a release in flight
(`check`), and merging that PR must publish nothing (`decide`); the `release:`
PR that cuts it at its current version is what publishes it.
"""

import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).resolve().parents[1] / "versions.py"
SPEC = importlib.util.spec_from_file_location("versions", SCRIPT)
versions = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(versions)

UNRELEASED = "# Changelog\n\n## [Unreleased]\n\n### Added\n\n- First cut.\n"
CUT = "# Changelog\n\n## [Unreleased]\n\n## [0.1.0] - 2026-10-31\n\n### Added\n\n- First cut.\n"


def manifest(name, version):
    return f'[package]\nname = "{name}"\nversion = "{version}"\n'


class NewCrateTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = self.tmp.name
        self.git("init", "-q")
        self.git("config", "user.email", "ci@example.invalid")
        self.git("config", "user.name", "CI test")
        self.git("config", "commit.gpgsign", "false")
        self.git("config", "tag.gpgsign", "false")
        # An already-released crate, `old` 0.2.0, tagged on the base commit.
        self.write("old/Cargo.toml", manifest("old", "0.2.0"))
        self.write("old/CHANGELOG.md", "# Changelog\n\n## [Unreleased]\n\n## [0.2.0] - 2026-10-01\n\n- x\n")
        self.write("Cargo.toml", '[workspace.dependencies]\nold = { path = "old", version = "0.2.0" }\n')
        self.commit("base")
        self.git("tag", "old-v0.2.0")

        self.ws = {
            "old": self.crate("old", (0, 2, 0)),
            "new": self.crate("new", (0, 1, 0)),
        }
        patches = [
            patch.object(versions, "ROOT", self.root),
            patch.object(versions, "workspace", lambda: self.ws),
            patch.object(versions, "on_crates_io", lambda name, version: False),
        ]
        for p in patches:
            p.start()
            self.addCleanup(p.stop)
        self.addCleanup(self.tmp.cleanup)

    # -- helpers -----------------------------------------------------------

    def git(self, *args):
        return subprocess.check_output(["git", *args], cwd=self.root, text=True).strip()

    def write(self, rel, text):
        path = os.path.join(self.root, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as f:
            f.write(text)

    def commit(self, msg):
        self.git("add", "-A")
        self.git("commit", "-qm", msg)

    def crate(self, name, version):
        return versions.Crate(
            name=name,
            version=version,
            dir=name,
            manifest=os.path.join(self.root, name, "Cargo.toml"),
            publish=True,
        )

    def add_new_crate(self):
        """The feature PR: `new` 0.1.0 with only `[Unreleased]`."""
        self.write("new/Cargo.toml", manifest("new", "0.1.0"))
        self.write("new/CHANGELOG.md", UNRELEASED)
        self.write(
            "Cargo.toml",
            '[workspace.dependencies]\nold = { path = "old", version = "0.2.0" }\n'
            'new = { path = "new", version = "0.1.0" }\n',
        )
        self.commit("feat: add new")

    def cut_first_release(self):
        """The `release:` PR: `[Unreleased]` -> `[0.1.0]`, version unchanged."""
        self.write("new/CHANGELOG.md", CUT)
        self.commit("release: new 0.1.0")

    def decide(self):
        out = io.StringIO()
        code = 0
        with contextlib.redirect_stdout(out):
            try:
                versions.cmd_decide([])
            except SystemExit as exc:
                code = exc.code
        return json.loads(out.getvalue()), code

    def tags(self):
        return versions.local_tags()

    # -- tests -------------------------------------------------------------

    def test_feature_merge_of_a_new_crate_is_not_in_flight_and_publishes_nothing(self):
        self.add_new_crate()
        new = self.ws["new"]
        self.assertTrue(versions.unreleased_new(self.tags(), new))
        self.assertEqual(versions.in_flight(self.ws, self.tags()), [])

        result, code = self.decide()
        self.assertEqual(code, 0)
        self.assertEqual(result["release"], [])
        self.assertEqual(result["errors"], [])
        self.assertEqual(result["unreleased_new"], ["new"])

    def test_release_pr_cutting_the_first_version_publishes_it(self):
        self.add_new_crate()
        self.cut_first_release()
        new = self.ws["new"]
        self.assertFalse(versions.unreleased_new(self.tags(), new))
        self.assertEqual([c.name for c in versions.in_flight(self.ws, self.tags())], ["new"])

        result, code = self.decide()
        self.assertEqual(code, 0)
        self.assertEqual([r["name"] for r in result["release"]], ["new"])
        self.assertEqual(result["release"][0]["tag"], "new-v0.1.0")
        # Added by an earlier (feature) commit: present at the base.
        self.assertEqual(result["release"][0]["previous"], "0.1.0")

    def test_a_new_crate_added_and_cut_in_one_commit_publishes(self):
        self.write("new/Cargo.toml", manifest("new", "0.1.0"))
        self.write("new/CHANGELOG.md", CUT)
        self.commit("release: new 0.1.0")
        result, code = self.decide()
        self.assertEqual(code, 0)
        self.assertEqual([r["name"] for r in result["release"]], ["new"])
        self.assertIsNone(result["release"][0]["previous"])

    def test_an_unfinished_first_release_is_reported_on_a_later_commit(self):
        self.add_new_crate()
        self.cut_first_release()
        self.write("old/src.rs", "// later work\n")
        self.commit("feat: unrelated")
        result, code = self.decide()
        self.assertEqual(code, 1)
        self.assertEqual(result["release"], [])
        self.assertEqual(len(result["errors"]), 1)
        self.assertIn("new 0.1.0 is not tagged", result["errors"][0])

    def test_once_tagged_the_crate_follows_the_normal_rules(self):
        self.add_new_crate()
        self.cut_first_release()
        self.git("tag", "new-v0.1.0")
        self.assertFalse(versions.never_tagged(self.tags(), "new"))
        self.assertEqual(versions.in_flight(self.ws, self.tags()), [])
        result, code = self.decide()
        self.assertEqual((result["release"], code), ([], 0))

    def test_plan_releases_a_new_crate_at_its_current_version(self):
        self.add_new_crate()
        bumps = versions.make_plan(self.ws, [("new", "0.1.0")], True)
        self.assertEqual(bumps["new"].new, (0, 1, 0))
        # Valid: never tagged, `[Unreleased]` has entries.
        with contextlib.redirect_stderr(io.StringIO()):
            versions.validate_plan(self.ws, bumps, self.tags())

    def test_plan_refuses_the_current_version_of_a_released_crate(self):
        self.add_new_crate()
        self.write("old/CHANGELOG.md", "# Changelog\n\n## [Unreleased]\n\n- y\n\n## [0.2.0] - 2026-10-01\n\n- x\n")
        self.commit("feat: old entry")
        bumps = versions.make_plan(self.ws, [("old", "0.2.0")], True)
        err = io.StringIO()
        with contextlib.redirect_stderr(err), self.assertRaises(SystemExit):
            versions.validate_plan(self.ws, bumps, self.tags())
        self.assertIn("only a crate never tagged", err.getvalue())


if __name__ == "__main__":
    unittest.main()
