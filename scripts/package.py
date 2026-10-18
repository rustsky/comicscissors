#!/usr/bin/env python3
"""Build the release bundle for one platform: the CLI binary plus the Calibre plugin zip.

    python3 scripts/package.py --binary target/release/comic-scissors --platform macos-universal

writes dist/comic-scissors-<version>-<platform>.tar.gz (.zip for Windows) containing

    comic-scissors[.exe]
    comic-scissors-calibre-plugin.zip   (with the same binary in bin/)
    README.md
"""

import argparse
import io
import re
import tarfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PLUGIN_DIR = ROOT / 'calibre_plugin'
PLUGIN_ZIP = 'comic-scissors-calibre-plugin.zip'
CALIBRE_PLATFORM = {'linux': 'linux', 'macos': 'osx', 'windows': 'windows'}


def cargo_version():
    m = re.search(r'^version\s*=\s*"([^"]+)"', (ROOT / 'Cargo.toml').read_text(), re.M)
    return m.group(1)


def zip_add(zf, name, data, executable=False):
    info = zipfile.ZipInfo(name, date_time=(2020, 1, 1, 0, 0, 0))
    info.compress_type = zipfile.ZIP_DEFLATED
    info.external_attr = (0o100755 if executable else 0o100644) << 16
    zf.writestr(info, data)


def plugin_zip(binary, exe, version, os_name):
    """The plugin zip as bytes, with version and platform written into __init__.py."""
    numbers = re.match(r'(\d+)\.(\d+)\.(\d+)', version).groups()
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, 'w') as zf:
        for path in sorted(PLUGIN_DIR.rglob('*')):
            if path.is_dir() or '__pycache__' in path.parts or path.name.startswith('.'):
                continue
            rel = path.relative_to(PLUGIN_DIR).as_posix()
            if rel.startswith('bin/'):
                continue
            data = path.read_bytes()
            if rel == '__init__.py':
                text = data.decode()
                text = re.sub(r'^(\s*version = ).*$', rf'\g<1>({", ".join(numbers)})', text, count=1, flags=re.M)
                text = re.sub(
                    r'^(\s*supported_platforms = ).*$', rf"\g<1>['{CALIBRE_PLATFORM[os_name]}']", text, count=1, flags=re.M
                )
                data = text.encode()
            zip_add(zf, rel, data)
        zip_add(zf, f'bin/{exe}', binary.read_bytes(), executable=True)
    return buf.getvalue()


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--binary', type=Path, required=True, help='built comic-scissors executable')
    ap.add_argument('--platform', required=True, help='e.g. linux-x86_64, macos-universal, windows-x86_64')
    ap.add_argument('--out', type=Path, default=ROOT / 'dist')
    ap.add_argument('--version', default=cargo_version(), help='default: version in Cargo.toml')
    args = ap.parse_args()

    os_name = args.platform.split('-')[0]
    if os_name not in CALIBRE_PLATFORM:
        ap.error(f'platform must start with one of {", ".join(CALIBRE_PLATFORM)}')
    exe = 'comic-scissors.exe' if os_name == 'windows' else 'comic-scissors'
    stem = f'comic-scissors-{args.version}-{args.platform}'
    args.out.mkdir(parents=True, exist_ok=True)

    files = [
        (exe, args.binary.read_bytes(), True),
        (PLUGIN_ZIP, plugin_zip(args.binary, exe, args.version, os_name), False),
        ('README.md', (ROOT / 'README.md').read_bytes(), False),
    ]
    if os_name == 'windows':
        archive = args.out / f'{stem}.zip'
        with zipfile.ZipFile(archive, 'w') as zf:
            for name, data, executable in files:
                zip_add(zf, f'{stem}/{name}', data, executable)
    else:
        archive = args.out / f'{stem}.tar.gz'
        with tarfile.open(archive, 'w:gz') as tf:
            for name, data, executable in files:
                info = tarfile.TarInfo(f'{stem}/{name}')
                info.size = len(data)
                info.mode = 0o755 if executable else 0o644
                info.mtime = 1577836800
                tf.addfile(info, io.BytesIO(data))
    print(archive)


if __name__ == '__main__':
    main()
