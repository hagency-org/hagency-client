"""Offline crypto/feed tests. Fixtures do not qualify Developer ID or notarization."""
import base64
from email.message import Message
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("macos_update", Path(__file__).with_name("macos-update.py"))
updates = importlib.util.module_from_spec(spec)
spec.loader.exec_module(updates)


class ReleaseVerification(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name)
        self.root = self.base / "assets"
        self.root.mkdir()
        self.key = self.base / "private.pem"
        subprocess.run([updates.OPENSSL, "genpkey", "-algorithm", "ed25519", "-out", str(self.key)], check=True, capture_output=True)
        self.public = base64.b64encode(subprocess.check_output([updates.OPENSSL, "pkey", "-in", str(self.key), "-pubout", "-outform", "DER"])[-32:]).decode()
        self.version, self.team = "0.7.0", "TESTTEAM01"
        for name in updates.expected_assets():
            (self.root / name).write_bytes(b"installer/checksum fixture")
        for arch in ("arm64",):
            dmg = self.root / f"hagency-macos-{arch}.dmg"
            signature = self.sign_file(dmg)
            (self.root / (dmg.name + ".sig")).write_text(signature + "\n")
            feed = self.root / f"appcast-macos-{arch}.xml"
            updates.write_feed(feed, self.version, arch, dmg.stat().st_size, signature, "Notes <>& 中文")
            record = {"schema": 1, "arch": arch, "version": self.version, "minimum_system_version": "12.0",
                "length": dmg.stat().st_size, "sha256": updates.digest(dmg), "signature": signature,
                "public_key": self.public, "team_id": self.team, "bundle_verified": True,
                "feed_sha256": updates.digest(feed)}
            self.write_record(arch, record)

    def sign_file(self, path):
        result = subprocess.check_output([updates.OPENSSL, "pkeyutl", "-sign", "-inkey", str(self.key), "-rawin", "-in", str(path)])
        return base64.b64encode(result).decode()

    def write_record(self, arch, record):
        path = self.root / f"macos-update-{arch}.json"
        path.write_text(json.dumps(record))
        (self.root / (path.name + ".sig")).write_text(self.sign_file(path) + "\n")

    def verify(self):
        updates.verify_complete(self.root, self.version, self.public, self.team)

    def test_complete_arm64_release_without_intel_assets_is_crypto_verified(self):
        self.verify()
        self.assertEqual(len(updates.expected_assets()), 18)
        self.assertFalse(any('macos-amd64' in name or 'macos-update-amd64' in name for name in updates.expected_assets()))
        feed = updates.ET.parse(self.root / "appcast-macos-arm64.xml")
        self.assertEqual(feed.findtext("./channel/item/description"), "Notes <>& 中文")

    def test_modified_archive_or_receipt_is_rejected(self):
        for name in ["hagency-macos-arm64.dmg", "macos-update-arm64.json", "appcast-macos-arm64.xml"]:
            path = self.root / name
            original = path.read_bytes()
            path.write_bytes(original + b"tampered")
            with self.assertRaises(ValueError):
                self.verify()
            path.write_bytes(original)
        self.verify()

    def test_wrong_public_key_or_bundle_version_is_rejected(self):
        with self.assertRaises(ValueError):
            updates.verify_complete(self.root, self.version, base64.b64encode(bytes(32)).decode(), self.team)
        with self.assertRaises(ValueError):
            updates.verify_complete(self.root, "0.8.0", self.public, self.team)

    def test_even_authenticated_metadata_cannot_select_a_mutable_or_foreign_url(self):
        feed = self.root / "appcast-macos-arm64.xml"
        feed.write_text(feed.read_text().replace("/download/v0.7.0/", "/latest/download/"))
        record = json.loads((self.root / "macos-update-arm64.json").read_text())
        record["feed_sha256"] = updates.digest(feed)
        self.write_record("arm64", record)
        with self.assertRaisesRegex(ValueError, "immutable DMG"):
            self.verify()

    def test_missing_or_unexpected_release_assets_are_rejected(self):
        extra = self.root / "unexpected-private-key"
        extra.touch()
        with self.assertRaises(ValueError):
            self.verify()
        extra.unlink()
        intel = self.root / "hagency-macos-amd64.dmg"
        intel.write_bytes(b"obsolete Intel installer")
        with self.assertRaises(ValueError):
            self.verify()
        intel.unlink()
        (self.root / "hagency-macos-arm64.dmg.sig").unlink()
        with self.assertRaises(ValueError):
            self.verify()

    def test_public_redirect_cache_content_type_and_archive_bytes_are_checked(self):
        def response(url, timeout):
            name = url.rsplit("/", 1)[1]
            stream = io.BytesIO((self.root / name).read_bytes())
            stream.headers = Message()
            stream.headers["Content-Type"] = "application/octet-stream"
            return stream
        with patch.object(updates.urllib.request, "urlopen", side_effect=response) as request:
            updates.verify_remote(self.root, self.version, self.public, self.team)
            self.assertEqual(request.call_count, 2)
            self.assertFalse(any('amd64' in call.args[0] for call in request.call_args_list))
        def stale(url, timeout):
            stream = response(url, timeout)
            stream.seek(0, 2)
            stream.write(b"stale")
            stream.seek(0)
            return stream
        with patch.object(updates.urllib.request, "urlopen", side_effect=stale), self.assertRaises(ValueError):
            updates.verify_remote(self.root, self.version, self.public, self.team)

    @unittest.skipUnless(sys.platform == "darwin" and os.environ.get("HAGENCY_SPARKLE_SDK"), "explicit pinned native Sparkle SDK integration")
    def test_pinned_sparkle_signer_uses_the_verified_raw_ed25519_format(self):
        seed = subprocess.check_output([updates.OPENSSL, "pkey", "-in", str(self.key), "-outform", "DER"])[-32:]
        private = self.base / "sparkle-key"
        private.touch(mode=0o600)
        private.write_bytes(base64.b64encode(seed))
        dmg = self.root / "hagency-macos-arm64.dmg"
        tool = Path(os.environ["HAGENCY_SPARKLE_SDK"]) / "bin/sign_update"
        for path in [dmg, self.root / "macos-update-arm64.json"]:
            signature = subprocess.check_output([str(tool), "--ed-key-file", str(private), "-p", str(path)], text=True).strip()
            updates.verify_signature(path, signature, self.public)


if __name__ == "__main__":
    unittest.main()
