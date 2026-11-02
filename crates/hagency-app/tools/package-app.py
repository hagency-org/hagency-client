"""Prepare app resources using the unified workspace's actual output path."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def prepare_app_resources(binary_name):
    manifest = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    namespace = manifest.get("lib", {}).get("name", manifest["package"]["name"].replace("-", "_"))
    source = ROOT / "resources"
    collected = ROOT / "dist/resources" / binary_name / "resources"
    # robius-packaging-commands 0.4.1 collects the app under its executable
    # name. Makepad crate_resource("self://...") uses the Rust crate name.
    # Validate the actual collector output before moving it to the namespace
    # declared in Cargo's packager resources; never mask an incomplete copy.
    for original in sorted(source.rglob("*")):
        relative = original.relative_to(source)
        if relative.parts[0] in ("android", "ios") or not original.is_file():
            continue
        packaged = collected / relative
        if not packaged.is_file() or packaged.is_symlink() or packaged.read_bytes() != original.read_bytes():
            raise RuntimeError(f"Missing or changed collected app resource: {relative.as_posix()}")
    if not collected.is_dir():
        raise RuntimeError("App resource collection is missing")
    if namespace != binary_name:
        destination = collected.parent.with_name(namespace)
        if destination.exists():
            raise RuntimeError(f"Duplicate app resource namespace: {namespace}")
        collected.parent.rename(destination)


def main():
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--no-deps", "--locked", "--format-version", "1"],
        cwd=ROOT, text=True,
    ))
    extension = ".exe" if os.name == "nt" else ""
    target = Path(metadata["target_directory"])
    # cargo-packager normally uses the host target. An explicit packaging
    # target can select the corresponding Cargo target subdirectory.
    triple = os.environ.get("CARGO_BUILD_TARGET")
    binary_dir = target / triple if triple else target
    binary = binary_dir / "release" / ("hagency" + extension)
    subprocess.run([
        "robius-packaging-commands", "before-each-package",
        "--force-makepad", "--binary-name", "hagency",
        "--path-to-binary", str(binary), "--add-deb-dep", "geoclue-2.0",
    ], cwd=ROOT, check=True)
    prepare_app_resources("hagency")
    subprocess.run([sys.executable, str(ROOT / "tools/package-octos.py"), "desktop"],
                   cwd=ROOT, check=True)


if __name__ == "__main__":
    main()
