#!/usr/bin/env python3
"""Sign final macOS DMGs, generate architecture feeds, and verify release assets.

The signing command requires the actual notarized app extracted from the final
DMG. The portable verifier independently checks Ed25519 signatures with the
configured public key; it never needs a private key or trusts a checksum alone.
"""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import subprocess
import sys
import tempfile
import urllib.request
import xml.etree.ElementTree as ET

NS = "http://www.andymatuschak.org/xml-namespaces/sparkle"
RELEASES = "https://github.com/hagency-org/hagency-client/releases"
OPENSSL = os.environ.get("HAGENCY_OPENSSL", "openssl")
MACOS_RELEASE_ARCHES = ("arm64",)


def require(condition, message):
    if not condition:
        raise ValueError(message)


def version_check(version):
    require(re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", version), "Expected a stable x.y.z version")


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def verify_signature(path, signature, public_key):
    public = base64.b64decode(public_key, validate=True)
    signed = base64.b64decode(signature, validate=True)
    require(len(public) == 32 and len(signed) == 64, "Invalid Sparkle key/signature length")
    with tempfile.TemporaryDirectory(prefix="hagency-sparkle-verify-") as directory:
        root = Path(directory)
        # RFC 8410 SubjectPublicKeyInfo for a raw Ed25519 public key.
        (root / "public.der").write_bytes(bytes.fromhex("302a300506032b6570032100") + public)
        (root / "signature").write_bytes(signed)
        result = subprocess.run([OPENSSL, "pkeyutl", "-verify", "-pubin", "-inkey", str(root / "public.der"),
            "-keyform", "DER", "-rawin", "-in", str(path), "-sigfile", str(root / "signature")], capture_output=True)
        require(result.returncode == 0, "Sparkle signature does not match the configured public key and final bytes")


def write_feed(path, version, arch, length, signature, notes):
    version_check(version)
    require(arch in ("arm64", "amd64"), "Invalid macOS architecture")
    require(len(notes.encode("utf-8")) <= 65536, "Release notes exceed 64 KiB")
    ET.register_namespace("sparkle", NS)
    rss = ET.Element("rss", version="2.0")
    channel = ET.SubElement(rss, "channel")
    ET.SubElement(channel, "title").text = "Hagency Client"
    item = ET.SubElement(channel, "item")
    ET.SubElement(item, "title").text = "Hagency " + version
    ET.SubElement(item, "description", {f"{{{NS}}}descriptionFormat": "plain-text"}).text = notes
    ET.SubElement(item, f"{{{NS}}}minimumSystemVersion").text = "12.0"
    ET.SubElement(item, "enclosure", {
        "url": f"{RELEASES}/download/v{version}/hagency-macos-{arch}.dmg",
        "length": str(length), "type": "application/octet-stream",
        f"{{{NS}}}version": version, f"{{{NS}}}shortVersionString": version,
        f"{{{NS}}}edSignature": signature,
    })
    ET.ElementTree(rss).write(path, encoding="utf-8", xml_declaration=True)


def inspect_bundle(bundle, version, arch, public_key, team):
    require(not bundle.is_symlink() and bundle.is_dir(), "Expected a real Hagency app in the DMG")
    info = plistlib.loads((bundle / "Contents/Info.plist").read_bytes())
    for key, expected in {
        "CFBundleIdentifier": "org.hagency.desktop", "CFBundleVersion": version,
        "CFBundleShortVersionString": version, "LSMinimumSystemVersion": "12.0",
        "SUPublicEDKey": public_key, "HagencySparkleEnabled": True,
        "SUAllowsAutomaticUpdates": False, "SUAutomaticallyUpdate": False,
        "SUFeedURL": f"{RELEASES}/latest/download/appcast-macos-{arch}.xml",
    }.items():
        require(key in info and type(info[key]) is type(expected) and info[key] == expected, f"Unexpected app bundle setting: {key}")
    executable = bundle / "Contents/MacOS/hagency"
    require(executable.resolve().is_relative_to(bundle.resolve()), "Executable escapes the bundle")
    actual = subprocess.check_output(["lipo", "-archs", str(executable)], text=True).split()
    require(actual == [{"arm64": "arm64", "amd64": "x86_64"}[arch]], "DMG executable architecture mismatch")
    rule = f'anchor apple generic and certificate leaf[subject.OU] = "{team}"'
    subprocess.run(["codesign", "--verify", "--strict", "--deep", "-R=" + rule, str(bundle)], check=True)
    subprocess.run(["codesign", "--verify", "--strict", "--deep", "-R=" + rule, str(bundle / "Contents/Frameworks/Sparkle.framework")], check=True)
    subprocess.run(["xcrun", "stapler", "validate", str(bundle)], check=True)
    subprocess.run(["spctl", "--assess", "--type", "execute", str(bundle)], check=True)


def sign(root, version, arch, public_key, team, sdk, notes):
    require(sys.platform == "darwin", "Signing requires a native macOS release runner")
    version_check(version)
    require(arch in MACOS_RELEASE_ARCHES and re.fullmatch(r"[A-Z0-9]{10}", team), "Invalid signing architecture/team")
    dmg = root / f"hagency-macos-{arch}.dmg"
    require(dmg.is_file() and not dmg.is_symlink(), "Final DMG is missing")
    subprocess.run(["codesign", "--verify", "--strict", "-R=" + f'anchor apple generic and certificate leaf[subject.OU] = "{team}"', str(dmg)], check=True)
    subprocess.run(["xcrun", "stapler", "validate", str(dmg)], check=True)
    subprocess.run(["spctl", "--assess", "--type", "open", "--context", "context:primary-signature", str(dmg)], check=True)
    mounted = plistlib.loads(subprocess.check_output(["hdiutil", "attach", "-plist", "-readonly", "-noautoopen", "-nobrowse", str(dmg)]))
    mounts = [entry["mount-point"] for entry in mounted["system-entities"] if "mount-point" in entry]
    try:
        require(len(mounts) == 1, "Expected one mounted installer volume")
        inspect_bundle(Path(mounts[0]) / "Hagency.app", version, arch, public_key, team)
    finally:
        for mount in mounts:
            subprocess.run(["hdiutil", "detach", mount], check=True)
    before = digest(dmg)
    # No Keychain fallback: the independent Sparkle secret must be supplied.
    secret = os.environ.get("HAGENCY_SPARKLE_PRIVATE_KEY", "").strip()
    require(len(base64.b64decode(secret, validate=True)) in (32, 64), "Missing/invalid Sparkle private key")
    with tempfile.TemporaryDirectory(prefix="hagency-sparkle-sign-") as directory:
        key = Path(directory) / "private-key"
        key.touch(mode=0o600)
        key.write_text(secret)
        def signed_file(path):
            result = subprocess.run([str(sdk / "bin/sign_update"), "--ed-key-file", str(key), "-p", str(path)], capture_output=True, text=True)
            require(result.returncode == 0, "Sparkle signing failed; check the protected signing configuration")
            signature = result.stdout.strip()
            verify_signature(path, signature, public_key)
            return signature
        signature = signed_file(dmg)
        require(digest(dmg) == before, "Final DMG changed while signing")
        (root / (dmg.name + ".sig")).write_text(signature + "\n")
        feed = root / f"appcast-macos-{arch}.xml"
        write_feed(feed, version, arch, dmg.stat().st_size, signature, notes)
        record = {"schema": 1, "arch": arch, "version": version, "minimum_system_version": "12.0",
            "length": dmg.stat().st_size, "sha256": before, "signature": signature,
            "public_key": public_key, "team_id": team, "bundle_verified": True, "feed_sha256": digest(feed)}
        metadata = root / f"macos-update-{arch}.json"
        metadata.write_text(json.dumps(record, indent=2) + "\n")
        (root / (metadata.name + ".sig")).write_text(signed_file(metadata) + "\n")
    verify_arch(root, version, arch, public_key, team)


def verify_arch(root, version, arch, public_key, team):
    version_check(version)
    dmg = root / f"hagency-macos-{arch}.dmg"
    metadata = root / f"macos-update-{arch}.json"
    require(metadata.stat().st_size <= 16384, "macOS release receipt is too large")
    verify_signature(metadata, (root / (metadata.name + ".sig")).read_text().strip(), public_key)
    record = json.loads(metadata.read_text())
    signature = (root / (dmg.name + ".sig")).read_text().strip()
    require(record == {"schema": 1, "arch": arch, "version": version, "minimum_system_version": "12.0",
        "length": dmg.stat().st_size, "sha256": digest(dmg), "signature": signature,
        "public_key": public_key, "team_id": team, "bundle_verified": True, "feed_sha256": digest(root / f"appcast-macos-{arch}.xml")}, "macOS release receipt mismatch")
    verify_signature(dmg, signature, public_key)
    feed = ET.parse(root / f"appcast-macos-{arch}.xml").getroot()
    items = feed.findall("./channel/item")
    require(feed.tag == "rss" and len(items) == 1, "Expected one stable update per architecture feed")
    require(items[0].findtext(f"{{{NS}}}minimumSystemVersion") == "12.0", "Wrong minimum macOS")
    enclosures = items[0].findall("enclosure")
    require(len(enclosures) == 1 and enclosures[0].attrib == {
        "url": f"{RELEASES}/download/v{version}/{dmg.name}", "length": str(record["length"]),
        "type": "application/octet-stream", f"{{{NS}}}version": version,
        f"{{{NS}}}shortVersionString": version, f"{{{NS}}}edSignature": signature,
    }, "Appcast does not describe the exact immutable DMG")
    return record


def expected_assets():
    installers = {"hagency-windows-amd64.exe"}
    installers |= {f"hagency-macos-{arch}.dmg" for arch in MACOS_RELEASE_ARCHES}
    installers |= {f"hagency-linux-{arch}{ext}" for arch in ("arm64", "amd64") for ext in (".deb", ".AppImage")}
    sums = {f"SHA256SUMS-{target}.txt" for target in ("windows-amd64", "macos-arm64", "linux-arm64", "linux-amd64")}
    windows = {"windows-update.json", "windows-update.json.sig", "hagency-windows-amd64.exe.sig"}
    mac = {name for arch in MACOS_RELEASE_ARCHES for name in (f"appcast-macos-{arch}.xml", f"macos-update-{arch}.json", f"macos-update-{arch}.json.sig", f"hagency-macos-{arch}.dmg.sig")}
    return installers | sums | windows | mac | {"SHA256SUMS.txt"}


def verify_complete(root, version, public_key, team):
    require({path.name for path in root.iterdir()} == expected_assets(), "Release assets are missing or unexpected")
    require(all(path.is_file() and not path.is_symlink() for path in root.iterdir()), "Release assets must be regular files")
    for arch in MACOS_RELEASE_ARCHES:
        verify_arch(root, version, arch, public_key, team)


def verify_remote(root, version, public_key, team):
    for arch in MACOS_RELEASE_ARCHES:
        record = verify_arch(root, version, arch, public_key, team)
        feed = f"appcast-macos-{arch}.xml"
        with urllib.request.urlopen(f"{RELEASES}/latest/download/{feed}", timeout=60) as response:
            require(response.headers.get_content_type() in ("application/octet-stream", "application/xml", "text/xml", "text/plain"), "Unexpected public feed Content-Type")
            require(response.read(1024 * 1024) == (root / feed).read_bytes(), "Latest feed redirect/cache differs from this release")
        actual = hashlib.sha256()
        length = 0
        with urllib.request.urlopen(f"{RELEASES}/download/v{version}/hagency-macos-{arch}.dmg", timeout=60) as response:
            require(response.headers.get_content_type() in ("application/octet-stream", "application/x-apple-diskimage"), "Unexpected public DMG Content-Type")
            for chunk in iter(lambda: response.read(1024 * 1024), b""):
                length += len(chunk)
                actual.update(chunk)
        require(length == record["length"] and actual.hexdigest() == record["sha256"], "Public immutable DMG differs from the verified bytes")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["sign", "verify", "verify-remote"])
    parser.add_argument("--root", type=Path, default=Path("release-assets"))
    parser.add_argument("--version", required=True)
    parser.add_argument("--arch", choices=MACOS_RELEASE_ARCHES)
    parser.add_argument("--notes", type=Path, default=Path("packaging/release-notes.md"))
    args = parser.parse_args()
    public_key, team = os.environ["HAGENCY_SPARKLE_PUBLIC_KEY"], os.environ["HAGENCY_SPARKLE_TEAM_ID"]
    if args.command == "sign":
        require(args.arch is not None, "Signing requires an architecture")
        sign(args.root, args.version, args.arch, public_key, team, Path(os.environ["HAGENCY_SPARKLE_SDK"]), args.notes.read_text())
    elif args.command == "verify":
        verify_complete(args.root, args.version, public_key, team)
    else:
        verify_remote(args.root, args.version, public_key, team)


if __name__ == "__main__":
    main()
