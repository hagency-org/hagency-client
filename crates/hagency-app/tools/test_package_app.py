"""Regression checks for resource collection with an external Cargo target."""
import builtins
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import tomllib
import plistlib
import shutil
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("package_desktop", Path(__file__).with_name("package-app.py"))
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)


class ResourcePaths(unittest.TestCase):
    def test_manifest_and_native_bundle_agree_on_application_identity(self):
        manifest = tomllib.loads((package.ROOT / "Cargo.toml").read_text(encoding="utf-8"))
        self.assertEqual(manifest["package"]["name"], "hagency-app")
        self.assertEqual(manifest["bin"], [{"name": "hagency", "path": "src/main.rs"}])
        config = manifest["package"]["metadata"]["packager"]
        self.assertEqual(config["product_name"], "Hagency")
        self.assertIn({"src": "./dist/resources/hagency_app", "target": "hagency_app"}, config["resources"])
        plist = plistlib.loads((package.ROOT / "packaging/macos/Info.plist").read_bytes())
        self.assertEqual(plist["CFBundleExecutable"], "hagency")
        self.assertEqual(plist["CFBundleDisplayName"], "Hagency")
        # Retain the release identity so installed accounts/OS grants keep working.
        self.assertEqual(plist["CFBundleIdentifier"], config["identifier"])
        extension = package.ROOT / "resources/android/java" / Path(*config["identifier"].split(".")) / "MakepadAppExtension.java"
        self.assertIn(f'package {config["identifier"]};', extension.read_text(encoding="utf-8"))

    def test_hook_imports_without_a_toml_library(self):
        # cargo-packager's child shell can select Python 3.10 even when the
        # outer Actions shell uses a newer Python. No third-party parser exists.
        original_import = builtins.__import__
        def without_toml(name, *args, **kwargs):
            if name in ("tomllib", "tomli", "toml"):
                raise ModuleNotFoundError("No TOML library on the packaging interpreter")
            return original_import(name, *args, **kwargs)
        probe_spec = importlib.util.spec_from_file_location("portable_package_hook", Path(package.__file__))
        probe = importlib.util.module_from_spec(probe_spec)
        with patch("builtins.__import__", side_effect=without_toml):
            probe_spec.loader.exec_module(probe)

    def collect(self, triple, damaged=None):
        with tempfile.TemporaryDirectory() as temporary:
            target = Path(temporary) / "external-cargo-target"
            root = Path(temporary) / "app"
            resources = root / "resources"
            (resources / "icons").mkdir(parents=True)
            (root / "Cargo.toml").write_text('[package]\nname="hagency-app"\n')
            (resources / "icons/send.svg").write_bytes(b"original send icon")
            (resources / "icon_128.png").write_bytes(b"original app image")
            (resources / "android").mkdir()
            (resources / "android/mobile-only").write_bytes(b"mobile")

            def prepare(command, **kwargs):
                if command[0] != "robius-packaging-commands":
                    return
                # The pinned 0.4.1 collector uses --binary-name, not the Rust
                # crate namespace, for this directory (the real release bug).
                collected = root / "dist/resources/hagency/resources"
                shutil.copytree(resources, collected, ignore=shutil.ignore_patterns("android"))
                if damaged == "missing":
                    (collected / "icons/send.svg").unlink()
                elif damaged == "changed":
                    (collected / "icons/send.svg").write_bytes(b"wrong icon bytes")

            with patch.dict(os.environ, {}, clear=True), \
                    patch.object(package, "ROOT", root), \
                    patch.object(package.subprocess, "check_output", return_value=json.dumps({"target_directory": str(target), "packages": [
                        {"manifest_path": str(root / "other/Cargo.toml"), "targets": [{"kind": ["lib"], "name": "wrong_library"}]},
                        {"manifest_path": str(root / "Cargo.toml"), "targets": [{"kind": ["bin"], "name": "hagency"}, {"kind": ["lib"], "name": "hagency_app"}]}
                    ]})), \
                    patch.object(package.subprocess, "run", side_effect=prepare) as run:
                if triple:
                    os.environ["CARGO_BUILD_TARGET"] = triple
                if damaged:
                    with self.assertRaisesRegex(RuntimeError, "icons/send.svg"):
                        package.main()
                    self.assertEqual(run.call_count, 1, "refuse before preparing other payloads")
                    return
                package.main()
                collection = run.call_args_list[0]
                command = collection.args[0]
                binary = Path(command[command.index("--path-to-binary") + 1])
                directory = target / triple if triple else target
                self.assertEqual(binary.parent, directory / "release")
                self.assertEqual(binary.stem, "hagency")
                self.assertEqual(command[command.index("--binary-name") + 1], "hagency")
                self.assertEqual(collection.kwargs["cwd"], root)
                self.assertEqual(Path(run.call_args_list[1].args[0][1]), root / "tools/package-octos.py")
                packaged = root / "dist/resources/hagency_app/resources"
                self.assertEqual((packaged / "icons/send.svg").read_bytes(), b"original send icon")
                self.assertEqual((packaged / "icon_128.png").read_bytes(), b"original app image")
                self.assertFalse((packaged / "android").exists())
                self.assertFalse((root / "dist/resources/hagency").exists())

    def test_external_host_target(self):
        self.collect(None)

    def test_explicit_target_triple(self):
        self.collect("aarch64-apple-darwin")

    def test_missing_collected_app_asset_refuses_packaging(self):
        self.collect(None, "missing")

    def test_changed_collected_app_asset_refuses_packaging(self):
        self.collect(None, "changed")


if __name__ == "__main__":
    unittest.main()
