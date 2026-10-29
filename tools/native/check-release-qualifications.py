#!/usr/bin/env python3
"""Reject unproven or stale real-host evidence before publishing installers.

This preflight does not execute models or replace the package-specific Rust
evidence validators. The release workflow runs those after this check succeeds.
"""
import json
from pathlib import Path
import re
import subprocess
import sys
import time
import tomllib

ROOT = Path(__file__).resolve().parents[2]
RECORDS = (
    ("crates/hagency-execution/qualification/codex-sandbox.json", "hagency-codex-sandbox-qualification-v1"),
    ("crates/hagency/qualification/two-agent.json", "hagency-two-agent-qualification-v1"),
)

COMPILED_PROMPTS = {
    "docs/workspace-claude-md-template.md",
    "docs/workspace-agents-md-template.md",
    "docs/workspace-supervisor-claude-template.md",
    "docs/workspace-supervisor-agents-template.md",
}


def code_input(name, embedded=()):
    """Conservatively bind build/runtime inputs; only explicit non-code may drift.

    Packaging templates, resources, toolchains and configuration are inputs too.
    A suffix allowlist would silently miss the next production input format.
    """
    path = Path(name)
    parts = path.parts
    if name in COMPILED_PROMPTS or name in embedded:
        return True
    if not parts or name in {record for record, _ in RECORDS}:
        return False
    if parts[0] in ("docs", "knowledge", "specs", "dev-vibe", ".agent-spec"):
        return False
    if name in ("LICENSE", "NOTICE", "rustfmt.toml", "typos.toml", ".gitignore", ".gitattributes"):
        return False
    # Shipped mini-app payloads remain inputs regardless of their extension.
    if parts[0] == "crates" and "bundle" in parts:
        return True
    if any(part in ("tests", "examples", "docs", "specs") for part in parts):
        return False
    if path.suffix == ".md":
        return False
    return True


def git(root, *args):
    result = subprocess.run(["git", *args], cwd=root, text=True, capture_output=True)
    if result.returncode:
        raise ValueError("Cannot verify qualification source revision against local Git history")
    return result.stdout


def embedded_inputs(root):
    """Direct Rust include_str/include_bytes paths override documentation rules.

    Generated/concat inputs remain bound by their build recipes and conservative
    resource classification; the four shipped Agent templates are also explicit.
    """
    sources = git(root, "ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", ":(glob)**/*.rs").split("\0")
    embedded = set()
    pattern = re.compile(r'\binclude_(?:str|bytes)!\s*\(\s*("(?:\\.|[^"\\])*")')
    for name in sources:
        if not code_input(name) or not (root / name).is_file():
            continue
        text = (root / name).read_text()
        for match in pattern.finditer(text):
            try:
                target = ((root / name).parent / json.loads(match[1])).resolve()
                embedded.add(target.relative_to(root.resolve()).as_posix())
            except (ValueError, OSError):
                raise ValueError(f"Cannot bind compiled input in {name} to this source tree") from None
    return embedded


def validate(root, now_ms=None):
    root = Path(root)
    now_ms = int(time.time() * 1000) if now_ms is None else now_ms
    version = tomllib.loads((root / "crates/hagency-app/Cargo.toml").read_text())["package"]["version"]
    errors = []
    try:
        embedded = embedded_inputs(root)
    except (OSError, ValueError) as error:
        return [str(error)]
    for name, schema in RECORDS:
        try:
            record = json.loads((root / name).read_text())
            if not isinstance(record, dict) or record.get("schema") != schema:
                raise ValueError("missing or unsupported evidence schema")
            if record.get("placeholder") is not False:
                raise ValueError("real-host qualification is missing or still a placeholder")
            commit = record.get("commit", "")
            if not isinstance(commit, str) or not re.fullmatch(r"[0-9a-f]{40}", commit):
                raise ValueError("missing exact source revision")
            captured = record.get("recorded_at_ms")
            if type(captured) is not int or not 0 < captured <= now_ms + 300_000:
                raise ValueError("invalid real-host capture timestamp")
            if schema == "hagency-two-agent-qualification-v1" and record.get("pinned_native_version") != version:
                raise ValueError("native qualification does not match the current desktop version")
            git(root, "cat-file", "-e", commit + "^{commit}")
            changed = git(root, "diff", "--name-only", "-z", "--no-renames", commit, "--").split("\0")
            changed += git(root, "ls-files", "-z", "--others", "--exclude-standard").split("\0")
            changed = sorted({name for name in changed if code_input(name, embedded)})
            if changed:
                raise ValueError("production/build inputs changed since qualification: " + ", ".join(changed[:8]))
        except (OSError, ValueError, TypeError, KeyError) as error:
            errors.append(f"{name}: {error}")
    return errors


def main():
    errors = validate(ROOT)
    if errors:
        print("Public release refused; retain the original unqualified state:", file=sys.stderr)
        for error in errors:
            print("- " + error, file=sys.stderr)
        return 1
    print("Qualification records match production inputs; package-specific verdict validation remains required.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
