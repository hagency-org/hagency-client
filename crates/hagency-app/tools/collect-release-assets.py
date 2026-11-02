"""Give verified installers their public, platform-neutral Hagency names."""
import argparse
import hashlib
from pathlib import Path
import shutil

EXTENSIONS = {'linux': ('.deb', '.AppImage'), 'macos': ('.dmg',), 'windows': ('.exe',)}


def collect(dist, output, platform, arch):
    if platform not in EXTENSIONS or arch not in ('amd64', 'arm64') or (platform == 'macos' and arch != 'arm64'):
        raise ValueError('Unsupported installer platform or architecture')
    installers = []
    for extension in EXTENSIONS[platform]:
        matches = list(dist.glob('*' + extension))
        if len(matches) != 1:
            raise ValueError(f'Expected one {extension} installer, got {matches}')
        installers.append((matches[0], f'hagency-{platform}-{arch}{extension}'))
    output.mkdir(parents=True, exist_ok=True)
    checksums = []
    for source, name in installers:
        target = output / name
        shutil.copy2(source, target)
        checksums.append(f'{hashlib.sha256(target.read_bytes()).hexdigest()}  {name}\n')
    (output / f'SHA256SUMS-{platform}-{arch}.txt').write_text(''.join(checksums), encoding='utf-8')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--platform', required=True, choices=EXTENSIONS)
    parser.add_argument('--arch', required=True, choices=('amd64', 'arm64'))
    parser.add_argument('--dist', type=Path, default=Path('dist'))
    parser.add_argument('--output', type=Path, default=Path('release-assets'))
    args = parser.parse_args()
    collect(args.dist, args.output, args.platform, args.arch)
