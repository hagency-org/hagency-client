"""Check release filenames and checksums without building installers."""
import hashlib
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('release_assets', Path(__file__).with_name('collect-release-assets.py'))
assets = importlib.util.module_from_spec(spec)
spec.loader.exec_module(assets)


class ReleaseAssets(unittest.TestCase):
    def test_every_release_target_has_complete_installers_and_valid_checksums(self):
        for platform, arch in (('windows', 'amd64'), ('macos', 'arm64'), ('linux', 'amd64'), ('linux', 'arm64')):
            with self.subTest(platform=platform, arch=arch), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                dist = root / 'dist'
                dist.mkdir()
                for extension in assets.EXTENSIONS[platform]:
                    (dist / ('installer' + extension)).write_bytes(b'installer fixture')
                output = root / 'release'
                assets.collect(dist, output, platform, arch)
                names = {f'hagency-{platform}-{arch}{extension}' for extension in assets.EXTENSIONS[platform]}
                sums = output / f'SHA256SUMS-{platform}-{arch}.txt'
                self.assertEqual({p.name for p in output.iterdir()}, names | {sums.name})
                for line in sums.read_text().splitlines():
                    digest, name = line.split('  ', 1)
                    self.assertEqual(digest, hashlib.sha256((output / name).read_bytes()).hexdigest())
                    self.assertEqual((output / name).read_bytes(), b'installer fixture')

    def test_macos_intel_installer_is_rejected_before_creating_release_output(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'installer.dmg').write_bytes(b'Intel installer fixture')
            output = root / 'release'
            with self.assertRaisesRegex(ValueError, 'Unsupported installer'):
                assets.collect(root, output, 'macos', 'amd64')
            self.assertFalse(output.exists())

    def test_missing_or_ambiguous_installers_fail_before_creating_release_output(self):
        for filenames in ((), ('one.deb',), ('one.exe', 'two.exe')):
            with self.subTest(files=filenames), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                for filename in filenames:
                    (root / filename).write_bytes(b'fixture')
                output = root / 'release'
                with self.assertRaises(ValueError):
                    assets.collect(root, output, 'linux' if 'one.deb' in filenames else 'windows', 'amd64')
                self.assertFalse(output.exists())


if __name__ == '__main__':
    unittest.main()
