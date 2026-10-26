"""Regression tests for CI coverage, using small dependency graphs and real Git."""

import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).resolve().parents[1] / "ci_affected.py"
SPEC = importlib.util.spec_from_file_location("ci_affected", SCRIPT)
ci = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ci)


def package(name, directory=None, deps=()):
    return {"id": name, "name": name,
            "manifest_path": f"/repo/{directory or name}/Cargo.toml",
            "dependencies": [{"name": d, "rename": None} if isinstance(d, str) else d for d in deps]}


def metadata(*packages):
    return {"workspace_root": "/repo", "workspace_members": [p["id"] for p in packages],
            "packages": list(packages)}


def lock(*packages):
    text = "version = 4\n"
    for name, version, deps in packages:
        text += f'[[package]]\nname = "{name}"\nversion = "{version}"\ndependencies = {json.dumps(deps)}\n'
    return text


class SelectionTests(unittest.TestCase):
    def setUp(self):
        self.graph = metadata(
            package("leaf", deps=["external"]),
            package("r2e-core", deps=["leaf", {"name": "devtools", "optional": True}]),
            package("r2e-compile-tests", deps=[{"name": "r2e-core", "kind": "dev"}]),
            package("app", "examples/app", ["r2e-core"]),
            package("unrelated"), package("devtools"),
        )

    def select(self, paths, before=None, after=None, graph=None):
        return ci.select(graph or self.graph, paths, (before or {}).__getitem__, (after or {}).__getitem__)

    def test_transitive_consumers_and_special_legs(self):
        result = self.select(["leaf/src/lib.rs"])
        self.assertEqual(result["packages"], ["app", "leaf", "r2e-compile-tests", "r2e-core"])
        self.assertTrue(result["dev_reload"])
        self.assertTrue(result["compile_tests"])
        self.assertFalse(result["full"])

    def test_optional_target_build_dev_and_renamed_dependencies(self):
        for details in ({"optional": True}, {"kind": "build"}, {"kind": "dev"},
                        {"target": "cfg(windows)"}, {"rename": "alias"}):
            with self.subTest(details=details):
                graph = metadata(package("a"), package("b", deps=[{"name": "a", **details}]))
                self.assertEqual(self.select(["a/src/lib.rs"], graph=graph)["packages"], ["a", "b"])

    def test_optional_devtools_change_enables_dev_reload(self):
        self.assertTrue(self.select(["devtools/src/lib.rs"])["dev_reload"])

    def test_nested_member_owns_its_files(self):
        graph = metadata(package("parent"), package("child", "parent/child"))
        self.assertEqual(self.select(["parent/child/src/lib.rs"], graph=graph)["packages"], ["child"])

    def test_leaf_application_omits_unrelated_legs(self):
        result = self.select(["examples/app/src/main.rs"])
        self.assertEqual(result["packages"], ["app"])
        self.assertFalse(result["dev_reload"])
        self.assertFalse(result["compile_tests"])

    def test_lock_only_transitive_upgrade(self):
        old = lock(("external", "1", ["transitive 1"]), ("transitive", "1", []))
        new = lock(("external", "1", ["transitive 2"]), ("transitive", "2", []))
        result = self.select(["Cargo.lock"], {"Cargo.lock": old}, {"Cargo.lock": new})
        self.assertEqual(result["packages"], ["app", "leaf", "r2e-compile-tests", "r2e-core"])
        self.assertFalse(result["full"])

    def test_removed_lock_dependency_uses_old_edges(self):
        old = lock(("external", "1", ["removed"]), ("removed", "1", []))
        new = lock(("external", "1", []))
        result = self.select(["Cargo.lock"], {"Cargo.lock": old}, {"Cargo.lock": new})
        self.assertIn("leaf", result["packages"])

    def test_lock_duplicate_versions_and_sources_are_conservative(self):
        old = lock(("external", "1", []), ("external", "2", []))
        new = old + 'source = "git+https://example.invalid/repo#new"\n'
        result = self.select(["Cargo.lock"], {"Cargo.lock": old}, {"Cargo.lock": new})
        self.assertIn("leaf", result["packages"])

    def test_lock_format_change_is_full(self):
        old = lock(("external", "1", []))
        new = old.replace("version = 4", "version = 3", 1)
        self.assertTrue(self.select(["Cargo.lock"], {"Cargo.lock": old}, {"Cargo.lock": new})["full"])

    def test_workspace_dependency_only_uses_consumer_alias(self):
        old = '[workspace.dependencies]\nalias = { package = "external", version = "1" }\n'
        new = old.replace('version = "1"', 'version = "2"')
        graph = metadata(package("consumer", deps=[{"name": "external", "rename": "alias"}]), package("other"))
        result = self.select(["Cargo.toml"], {"Cargo.toml": old}, {"Cargo.toml": new}, graph)
        self.assertEqual(result["packages"], ["consumer"])

    def test_global_settings_and_unknown_inputs_are_full(self):
        for path in [".cargo/config.toml", "rust-toolchain.toml", ".github/workflows/test.yml",
                     "scripts/ci_affected.py", "shared/fixture.json"]:
            with self.subTest(path=path):
                self.assertTrue(self.select([path])["full"])
        self.assertTrue(self.select(["Cargo.toml"], {"Cargo.toml": '[workspace]\nresolver="2"'},
                                    {"Cargo.toml": '[workspace]\nresolver="3"'})["full"])

    def test_deleted_manifest_is_full(self):
        with patch.object(Path, "read_text", side_effect=FileNotFoundError):
            result = ci.select(self.graph, ["leaf/Cargo.toml"], None, Path.read_text)
        self.assertTrue(result["full"])

    def test_docs_and_embedded_inputs(self):
        graph = metadata(package("r2e-cli"), package("llm-doctests"), package("other"))
        self.assertFalse(self.select(["docs/book/src/intro.md"], graph=graph)["run"])
        self.assertFalse(self.select([], graph=graph)["run"])
        self.assertEqual(self.select(["llm/config.md"], graph=graph)["packages"], ["llm-doctests", "r2e-cli"])
        self.assertEqual(self.select(["docs/features/01-configuration.md"], graph=graph)["packages"], ["r2e-cli"])

    def test_cycles_terminate(self):
        self.assertEqual(ci.closure({"a"}, {"a": {"b"}, "b": {"a"}}), {"a", "b"})


class CommandTests(unittest.TestCase):
    def test_full_commands_match_existing_ci(self):
        selection = ci.plan(full=True, reason="manual")
        self.assertEqual(ci.cargo_command(selection, "test"),
                         ["cargo", "test", "--workspace", "--exclude", "r2e-compile-tests"])
        self.assertEqual(ci.cargo_command(selection, "msrv", "1.94"),
                         ["cargo", "+1.94", "check", "--all-targets", "--workspace"])

    def test_no_packages_never_accidentally_runs_default_members(self):
        selection = ci.plan(reason="docs")
        for phase in ["test", "msrv", "compile-tests"]:
            self.assertEqual(ci.cargo_command(selection, phase), [])
        only_compile = ci.plan(["r2e-compile-tests"], reason="fixture")
        self.assertEqual(ci.cargo_command(only_compile, "test"), [])
        self.assertEqual(ci.cargo_command(only_compile, "compile-tests"),
                         ["cargo", "test", "-p", "r2e-compile-tests"])

    def test_selected_features_preserve_workspace_test_gates(self):
        graph = metadata(package("a"), package("b"))
        graph["resolve"] = {"nodes": [{"id": "a", "features": ["enabled-by-example"]},
                                      {"id": "b", "features": ["unrelated"]}]}
        command = ci.cargo_command(ci.plan(["a"], reason="source"), "test", metadata=graph)
        self.assertEqual(command, ["cargo", "test", "-p", "a", "--features", "a/enabled-by-example"])


class GitTests(unittest.TestCase):
    def test_rename_between_crates_selects_old_and_new_owner(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def git(*args):
                return subprocess.check_output(["git", *args], cwd=root, text=True).strip()
            git("init", "-q")
            git("config", "user.email", "ci@example.invalid")
            git("config", "user.name", "CI test")
            (root / "a").mkdir()
            (root / "b").mkdir()
            (root / "a/file.rs").write_text("// moved source\n")
            git("add", ".")
            git("commit", "-qm", "base")
            base = git("rev-parse", "HEAD")
            git("mv", "a/file.rs", "b/file.rs")
            git("commit", "-qm", "move")
            graph = metadata(package("a"), package("b"), package("consumer", deps=["a"]))
            real_output = ci.output
            def output(*args, **kwargs):
                return json.dumps(graph) if args[0] == "cargo" else real_output(*args, **kwargs)
            with patch.object(ci, "output", side_effect=output):
                self.assertEqual(ci.git_plan(root, base)["packages"], ["a", "b", "consumer"])

    def test_missing_base_falls_back_to_full(self):
        env = dict(os.environ)
        env.pop("GITHUB_STEP_SUMMARY", None)
        result = subprocess.run(["python3", str(SCRIPT), "plan", "--base", "0" * 40],
                                cwd=SCRIPT.parents[1], env=env, text=True, capture_output=True, check=True)
        self.assertTrue(json.loads(result.stdout)["full"])


class CargoTests(unittest.TestCase):
    def test_runner_preserves_feature_gated_tests_without_testing_unrelated_member(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Cargo.toml").write_text('[workspace]\nmembers=["lib", "app"]\nresolver="3"\n')
            for name in ("lib", "app"):
                (root / name / "src").mkdir(parents=True)
                (root / name / "Cargo.toml").write_text(
                    f'[package]\nname="{name}"\nversion="0.1.0"\nedition="2024"\n'
                    + ('[features]\nextra=[]\n' if name == "lib" else
                       '[dependencies]\nlib={path="../lib",features=["extra"]}\n'))
            (root / "lib/src/lib.rs").write_text("")
            (root / "lib/tests").mkdir()
            (root / "lib/tests/feature.rs").write_text(
                '#[cfg(feature="extra")]\n#[test]\nfn workspace_feature_is_preserved() {}\n')
            # If selection accidentally invokes the whole workspace, this fails.
            (root / "app/src/lib.rs").write_text('compile_error!("unselected package was built");\n')
            env = dict(os.environ, RUSTC_WRAPPER="", CARGO_TARGET_DIR=str(root / "target"),
                       CARGO_NET_OFFLINE="true", CI_PLAN=json.dumps(ci.plan(["lib"], reason="fixture")))
            subprocess.run(["cargo", "generate-lockfile", "--offline"], cwd=root, env=env,
                           check=True, capture_output=True)
            for phase in ("test", "msrv"):
                result = subprocess.run(["python3", str(SCRIPT), "run", phase], cwd=root, env=env,
                                        text=True, capture_output=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                if phase == "test":
                    self.assertIn("workspace_feature_is_preserved ... ok", result.stdout)


if __name__ == "__main__":
    unittest.main()
