#!/usr/bin/env python3
"""Check or bump the committed release version without changing dependencies."""

import argparse
import re
import subprocess
from pathlib import Path

import tomllib

ROOT = Path(__file__).resolve().parents[1]
VERSION_PATTERN = re.compile(r'(\bversion\s*=\s*")([^"]+)(";)')
SEMVER = re.compile(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)')


def flake_version(source: str) -> str:
    matches = list(VERSION_PATTERN.finditer(source))
    if len(matches) != 1:
        raise ValueError('expected exactly one literal version in flake.nix')
    version = matches[0][2]
    if not SEMVER.fullmatch(version):
        raise ValueError('release version must be MAJOR.MINOR.PATCH')
    return version


def check(root: Path) -> str:
    version = flake_version((root / 'flake.nix').read_text())
    manifest = tomllib.loads((root / 'Cargo.toml').read_text())
    lock = tomllib.loads((root / 'Cargo.lock').read_text())
    package = [p for p in lock['package'] if p['name'] == 'dictator' and 'source' not in p]
    if len(package) != 1 or manifest['package']['version'] != version or package[0]['version'] != version:
        raise ValueError('flake.nix, Cargo.toml and Cargo.lock versions must agree')
    return version


def bump(root: Path, version: str) -> None:
    current = check(root)
    if not SEMVER.fullmatch(version) or tuple(map(int, version.split('.'))) <= tuple(map(int, current.split('.'))):
        raise ValueError('new version must be MAJOR.MINOR.PATCH and greater than the current version')
    replacements = {
        'flake.nix': VERSION_PATTERN.sub(lambda m: m[1] + version + m[3], (root / 'flake.nix').read_text()),
    }
    for name in ('Cargo.toml', 'Cargo.lock'):
        source = (root / name).read_text()
        pattern = re.compile(r'(name = "dictator"\nversion = ")[^"]+(")')
        updated, count = pattern.subn(lambda m: m[1] + version + m[2], source)
        if count != 1:
            raise ValueError(f'expected one dictator package in {name}')
        replacements[name] = updated
    for name, source in replacements.items():
        (root / name).write_text(source)
    check(root)


def git(root: Path, *args: str) -> str:
    return subprocess.check_output(['git', '-C', str(root), *args], text=True).strip()


def release_needed(root: Path, version: str) -> bool:
    head = git(root, 'rev-parse', 'HEAD')
    tag = f'refs/tags/v{version}'
    exists = subprocess.run(['git', '-C', str(root), 'show-ref', '--verify', '--quiet', tag], check=False)
    if exists.returncode == 1:
        return True
    exists.check_returncode()
    tagged = git(root, 'rev-parse', f'{tag}^{{commit}}')
    if tagged == head:
        return True  # Idempotent publishing resumes this exact release.
    ancestor = subprocess.run(['git', '-C', str(root), 'merge-base', '--is-ancestor', tagged, head], check=False)
    if ancestor.returncode != 0:
        raise ValueError(f'v{version} already belongs to a different history; refusing to reuse it')
    # Ordinary development may retain a released version. A version change back
    # to an old tag is a conflict, even when that tag is an ancestor.
    parent = git(root, 'show', 'HEAD^:flake.nix')
    if flake_version(parent) != version:
        raise ValueError(f'v{version} already exists; bump to an unused version')
    return False


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group()
    group.add_argument('--bump', metavar='VERSION')
    group.add_argument('--release-plan', action='store_true')
    args = parser.parse_args()
    try:
        if args.bump:
            bump(ROOT, args.bump)
        version = check(ROOT)
        if args.release_plan:
            print(f'version={version}')
            print(f'release={str(release_needed(ROOT, version)).lower()}')
        else:
            print(version)
    except ValueError as error:
        parser.exit(1, f'{error}\n')


if __name__ == '__main__':
    main()
