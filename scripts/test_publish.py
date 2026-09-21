"""Run the publisher against a local Git remote and a simulated GitHub API."""

import json
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

import version


class PublishTests(unittest.TestCase):
    root: Path
    work: Path
    env: dict[str, str]

    def setUp(self) -> None:
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.work = Path(temp.name)
        self.root = self.work / 'repo'
        self.root.mkdir()
        (self.root / 'scripts').mkdir()
        for name in ('version.py', 'publish-release.sh'):
            shutil.copy2(Path(__file__).parent / name, self.root / 'scripts' / name)
        (self.root / 'flake.nix').write_text('{ version = "2.5.0"; }')
        (self.root / 'Cargo.toml').write_text('[package]\nname = "dictator"\nversion = "2.5.0"\n')
        (self.root / 'Cargo.lock').write_text('version = 4\n[[package]]\nname = "dictator"\nversion = "2.5.0"\n')
        for args in (('init', '-q'), ('config', 'user.email', 'test@example.com'),
                     ('config', 'user.name', 'Test'), ('add', '.'), ('commit', '-qm', 'release')):
            version.git(self.root, *args)
        subprocess.run(['git', 'init', '--bare', '-q', str(self.work / 'remote')], check=True)
        version.git(self.root, 'remote', 'add', 'origin', str(self.work / 'remote'))
        (self.root / 'release-assets').mkdir()
        for name in ('dictator.tar.gz', 'SHA256SUMS'):
            (self.root / 'release-assets' / name).touch()
        fakebin = self.work / 'bin'
        fakebin.mkdir()
        gh = fakebin / 'gh'
        gh.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys
with pathlib.Path(os.environ['CALLS']).open('a') as log:
    log.write(json.dumps(sys.argv[1:]) + '\\n')
if sys.argv[1] == 'api':
    if os.environ.get('FAIL_API') == '1':
        sys.exit(1)
    print(os.environ.get('RELEASE_STATE', ''))
if sys.argv[1:3] == ['release', 'upload'] and os.environ.get('FAIL_UPLOAD') == '1':
    sys.exit(1)
''')
        gh.chmod(0o755)
        self.env = dict(os.environ, PATH=f'{fakebin}:{os.environ["PATH"]}', VERSION='2.5.0',
                        GITHUB_SHA=version.git(self.root, 'rev-parse', 'HEAD'),
                        GITHUB_REPOSITORY='example/dictator', CALLS=str(self.work / 'calls'))

    def publish(self, **env: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(['bash', 'scripts/publish-release.sh'], cwd=self.root,
                              env=self.env | env, text=True, capture_output=True, check=False)

    def calls(self) -> list[list[str]]:
        path = self.work / 'calls'
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def test_new_release_tags_tested_commit_and_publishes_after_upload(self) -> None:
        result = self.publish()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(version.git(self.root, 'rev-parse', 'v2.5.0^{commit}'), self.env['GITHUB_SHA'])
        self.assertEqual([call[:2] for call in self.calls()][1:],
                         [['release', 'create'], ['release', 'upload'], ['release', 'edit']])

    def test_failed_upload_leaves_draft_and_retry_resumes(self) -> None:
        self.assertNotEqual(self.publish(FAIL_UPLOAD='1').returncode, 0)
        self.assertNotIn(['release', 'edit'], [call[:2] for call in self.calls()])
        (self.work / 'calls').unlink()
        result = self.publish(RELEASE_STATE='true')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([call[:2] for call in self.calls()][1:], [['release', 'upload'], ['release', 'edit']])

    def test_published_release_is_untouched(self) -> None:
        self.assertEqual(self.publish(RELEASE_STATE='false').returncode, 0)
        self.assertEqual(len(self.calls()), 1)

    def test_api_error_never_creates_or_publishes_release(self) -> None:
        self.assertNotEqual(self.publish(FAIL_API='1').returncode, 0)
        self.assertEqual(len(self.calls()), 1)

    def test_conflicting_tag_never_calls_github(self) -> None:
        version.git(self.root, 'tag', 'v2.5.0')
        version.git(self.root, 'commit', '-qm', 'later', '--allow-empty')
        self.env['GITHUB_SHA'] = version.git(self.root, 'rev-parse', 'HEAD')
        self.assertNotEqual(self.publish().returncode, 0)
        self.assertEqual(self.calls(), [])


if __name__ == '__main__':
    unittest.main()
