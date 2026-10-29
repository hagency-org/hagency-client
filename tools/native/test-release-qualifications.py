#!/usr/bin/env python3
"""Real Git history tests for release qualification freshness and refusal."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("qualifications", Path(__file__).with_name("check-release-qualifications.py"))
qualifications = importlib.util.module_from_spec(spec)
spec.loader.exec_module(qualifications)


class QualificationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="hagency qualification ")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.git("init", "--quiet", "--initial-branch=main")
        self.git("config", "user.name", "Fixture")
        self.git("config", "user.email", "fixture@example.invalid")
        self.git("config", "commit.gpgsign", "false")
        self.git("config", "core.hooksPath", str(self.root / "no-hooks"))
        self.write("crates/hagency-app/Cargo.toml", '[package]\nname="app"\nversion="0.6.0"\n')
        self.write("crates/hagency-app/src/lib.rs", 'const PROMPT: &str = include_str!("../../../docs/fixture-instructions.md");\npub fn original() {}\n')
        self.write("docs/fixture-instructions.md", "original instructions\n")
        self.write("Cargo.lock", "version = 4\n")
        self.git("add", ".")
        self.git("commit", "--quiet", "-m", "qualified production inputs")
        self.commit = self.git("rev-parse", "HEAD").strip()
        self.records = []
        for name, schema in qualifications.RECORDS:
            record = {"schema": schema, "placeholder": False, "commit": self.commit,
                      "recorded_at_ms": 1_000, "pinned_native_version": "0.6.0"}
            self.records.append(record)
            self.write(name, json.dumps(record))

    def git(self, *args):
        return subprocess.check_output(["git", *args], cwd=self.root, text=True, stderr=subprocess.PIPE)

    def write(self, name, text):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def verify(self):
        return qualifications.validate(self.root, now_ms=2_000)

    def test_current_inputs_pass_but_this_does_not_replace_the_rust_claim_validators(self):
        self.assertEqual(self.verify(), [])
        self.write("docs/progress.md", "evidence only\n")
        self.git("add", ".")
        self.git("commit", "--quiet", "-m", "record real-host evidence")
        self.assertNotEqual(self.commit, self.git("rev-parse", "HEAD").strip())
        self.assertEqual(self.verify(), [], "recording evidence must not require a fixed-point commit hash")

    def test_missing_placeholder_malformed_unknown_revision_and_wrong_version_are_refused(self):
        name, _ = qualifications.RECORDS[1]
        original = self.records[1]
        for changes in ({"placeholder": True}, {"placeholder": None}, {"schema": "wrong"},
                        {"commit": "0" * 40}, {"recorded_at_ms": True},
                        {"recorded_at_ms": 999_999_999}, {"pinned_native_version": "0.1.0"}):
            with self.subTest(changes=changes):
                self.write(name, json.dumps(dict(original, **changes)))
                self.assertTrue(self.verify())
        self.write(name, "not json")
        self.assertTrue(self.verify())
        (self.root / name).unlink()
        self.assertTrue(self.verify())

    def test_embedded_markdown_is_code_even_when_ordinary_progress_docs_are_not(self):
        for name in qualifications.COMPILED_PROMPTS:
            self.assertTrue(qualifications.code_input(name), name)
        self.write("docs/progress.md", "ordinary evidence notes\n")
        self.assertEqual(self.verify(), [])
        self.write("docs/fixture-instructions.md", "changed compiled instructions\n")
        self.assertTrue(self.verify(), "literal compiler inputs override the docs exception")

    def test_modified_deleted_new_source_and_dependency_inputs_invalidate_old_evidence(self):
        for name, text in (("crates/hagency-app/src/lib.rs", "pub fn changed() {}\n"),
                           ("Cargo.lock", "version = 5\n"),
                           ("crates/hagency-app/src/new.rs", "pub fn added() {}\n"),
                           ("crates/hagency-app/build.rs", "fn main() {}\n"),
                           ("crates/hagency-app/packaging/octos.lock.json", "{}"),
                           ("crates/hagency-app/tools/package-octos.py", "changed_recipe=True\n"),
                           (".cargo/config.toml", "[build]\n"),
                           (".github/workflows/engine-checks.yml", "changed: true\n"),
                           ("rust-toolchain.toml", "[toolchain]\nchannel='1.99'\n"),
                           ("crates/hagency-app/packaging/macos/Entitlements.plist", "changed entitlement"),
                           ("crates/hagency-app/packaging/macos/Info.plist", "changed bundle"),
                           ("crates/hagency-app/packaging/windows/installer.nsi", "changed installer"),
                           ("crates/hagency-app/resources/i18n/zh-CN.json", "changed translations"),
                           ("crates/hagency-app/resources/new-format.unknown", "new production format")):
            with self.subTest(name=name):
                path = self.root / name
                previous = path.read_text() if path.exists() else None
                self.write(name, text)
                self.assertTrue(self.verify(), name)
                if previous is None:
                    path.unlink()
                else:
                    path.write_text(previous)
        (self.root / "crates/hagency-app/src/lib.rs").unlink()
        self.assertTrue(self.verify())


if __name__ == "__main__":
    unittest.main()
