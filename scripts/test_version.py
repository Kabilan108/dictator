"""Exercise version changes and tag conflicts in disposable repositories."""

import tempfile
import unittest
from pathlib import Path

import version


class VersionTests(unittest.TestCase):
    root: Path
    temp: tempfile.TemporaryDirectory[str]

    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / 'flake.nix').write_text('{ version = "2.4.0"; }\n')
        (self.root / 'Cargo.toml').write_text('[package]\nname = "dictator"\nversion = "2.4.0"\n')
        (self.root / 'Cargo.lock').write_text(
            'version = 4\n[[package]]\nname = "dictator"\nversion = "2.4.0"\n'
            '[[package]]\nname = "dependency"\nversion = "2.4.0"\n'
        )
        self.git('init', '-q')
        self.git('config', 'user.email', 'test@example.com')
        self.git('config', 'user.name', 'Test')
        self.commit()

    def git(self, *args: str) -> str:
        return version.git(self.root, *args)

    def commit(self) -> None:
        self.git('add', '.')
        self.git('commit', '-qm', 'fixture', '--allow-empty')

    def test_bump_changes_only_project_versions(self) -> None:
        version.bump(self.root, '2.5.0')
        self.assertEqual(version.check(self.root), '2.5.0')
        self.assertIn('name = "dependency"\nversion = "2.4.0"', (self.root / 'Cargo.lock').read_text())

    def test_invalid_or_non_increasing_bump_leaves_files_unchanged(self) -> None:
        before = {p.name: p.read_text() for p in self.root.glob('*') if p.is_file()}
        for invalid in ('2.4.0', '2.3.0', '02.5.0', '2.5.0;echo bad', 'v2.5.0'):
            with self.assertRaises(ValueError):
                version.bump(self.root, invalid)
        self.assertEqual(before, {p.name: p.read_text() for p in self.root.glob('*') if p.is_file()})

    def test_mismatch_fails_before_mutation(self) -> None:
        path = self.root / 'Cargo.toml'
        path.write_text(path.read_text().replace('2.4.0', '2.3.0'))
        with self.assertRaises(ValueError):
            version.bump(self.root, '2.5.0')
        self.assertIn('2.4.0', (self.root / 'flake.nix').read_text())

    def test_new_version_and_exact_tag_retry(self) -> None:
        self.assertTrue(version.release_needed(self.root, '2.4.0'))
        self.git('tag', '-a', 'v2.4.0', '-m', 'release')
        self.assertTrue(version.release_needed(self.root, '2.4.0'))

    def test_development_after_release_only_caches(self) -> None:
        self.git('tag', 'v2.4.0')
        self.commit()
        self.assertFalse(version.release_needed(self.root, '2.4.0'))

    def test_reused_old_version_rejected(self) -> None:
        self.git('tag', 'v2.4.0')
        version.bump(self.root, '2.5.0')
        self.commit()
        self.git('checkout', 'HEAD^', '--', 'flake.nix', 'Cargo.toml', 'Cargo.lock')
        self.commit()
        with self.assertRaises(ValueError):
            version.release_needed(self.root, '2.4.0')

    def test_tag_on_different_history_rejected(self) -> None:
        base = self.git('rev-parse', 'HEAD')
        self.commit()
        self.git('tag', 'v2.4.0')
        self.git('checkout', '--detach', base)
        with self.assertRaises(ValueError):
            version.release_needed(self.root, '2.4.0')


if __name__ == '__main__':
    unittest.main()
