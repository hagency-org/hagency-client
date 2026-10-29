"""Validate the monorepo boundary and local dependency graph without compiling."""
import json
from pathlib import Path
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def main():
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--no-deps", "--locked", "--format-version", "1"],
        cwd=ROOT, text=True,
    ))
    members = set(metadata["workspace_members"])
    packages = [p for p in metadata["packages"] if p["id"] in members]
    assert packages, "Empty workspace"
    workspace_manifests = {Path(p["manifest_path"]).resolve() for p in packages}
    root_manifest = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    excluded = set(root_manifest["workspace"].get("exclude", []))
    vendored_manifests = {}
    for name, patch in root_manifest.get("patch", {}).get("crates-io", {}).items():
        if not isinstance(patch, dict) or "path" not in patch:
            continue
        relative = Path(patch["path"])
        if relative.parts[0] != "vendor":
            continue
        assert not relative.is_absolute() and ".." not in relative.parts, relative
        assert relative.as_posix() in excluded, f"Vendor patch not explicitly excluded: {relative}"
        directory = ROOT / relative
        assert directory == directory.resolve(), f"Symlinked vendor patch directory: {relative}"
        assert directory.is_relative_to(ROOT / "vendor"), relative
        manifest = directory / "Cargo.toml"
        assert not manifest.is_symlink(), f"Symlinked vendor manifest: {relative}"
        assert manifest not in workspace_manifests, f"Vendor patch is a workspace member: {relative}"
        vendored_manifests[manifest] = patch.get("package", name)
    for package in packages:
        manifest = Path(package["manifest_path"]).resolve()
        assert manifest.parent.parent == ROOT / "crates", manifest
        assert not (manifest.parent / "Cargo.lock").exists(), manifest
    app = next(p for p in packages if p["name"] == "hagency-app")
    assert Path(app["manifest_path"]).parent == ROOT / "crates/hagency-app"
    assert app["default_run"] == "hagency"
    assert [t["name"] for t in app["targets"] if "bin" in t["kind"]] == ["hagency"]
    engine = next(p for p in app["dependencies"] if p["name"] == "hagency")
    assert engine["source"] is None and Path(engine["path"]).resolve() == ROOT / "crates/hagency"
    files = subprocess.check_output([
        "git", "ls-files", "--cached", "--others", "--exclude-standard", "-z",
    ], cwd=ROOT).decode().split("\0")
    for name in files:
        if not name.endswith("Cargo.toml") or not (ROOT / name).is_file():
            continue
        data = tomllib.loads((ROOT / name).read_text(encoding="utf-8"))
        if "package" in data:
            # Match the registered manifest itself, never an unregistered alias
            # whose symlink happens to resolve to an allowed package.
            manifest = ROOT / name
            if manifest in vendored_manifests:
                assert data["package"]["name"] == vendored_manifests[manifest], f"Vendor patch name mismatch: {name}"
            else:
                assert Path(name).parts[0] == "crates", f"Crate outside crates/: {name}"
            assert "workspace" not in data, f"Nested workspace: {name}"
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text())
    assert not any("github.com/chrislearn/hagency-client" in p.get("source", "") for p in lock["package"]), "Duplicate Git engine"
    engine_crate = next(p for p in packages if p["name"] == "hagency")
    assert engine_crate["default_run"] is None
    assert not any(t["name"] == "hagency-cli" for t in engine_crate["targets"])
    binaries = [t["name"] for p in packages for t in p["targets"] if "bin" in t["kind"]]
    assert len(binaries) == len(set(binaries)), "Colliding workspace executable names"
    print(json.dumps({"packages": len(packages), "app_engine": engine["path"],
                      "target_directory": metadata["target_directory"],
                      "vendored_patches": sorted(p.relative_to(ROOT).as_posix() for p in vendored_manifests)}))


if __name__ == "__main__":
    main()
