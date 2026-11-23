import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from selfdev_release_highlights import bullets

SCRIPT = Path(__file__).with_name('selfdev_release_highlights.py').resolve()


class HighlightsTests(unittest.TestCase):
    def test_public_text_is_bounded_plain_bullets(self):
        result = bullets('- <script> @everyone\n* `example`\u202e\n' + ('z' * 500 + '\n') * 20)
        self.assertEqual(result[:2], ['- &lt;script&gt; ＠everyone', r'- \`example\`'])
        self.assertEqual(len(result), 8)
        self.assertEqual(len(result[-1]), 362)

    def test_truncation_preserves_words_and_balanced_backticks(self):
        long_note = (
            'Provider fallback now skips exhausted accounts across '
            + ' '.join(f'`backend-{i}`' for i in range(45))
            + ' cleanly.'
        )
        items = bullets(long_note)
        self.assertEqual(len(items), 1)
        item = items[0]
        self.assertTrue(item.endswith('…'))
        # Unescaped backticks must stay balanced (even count) and never slice mid-word.
        self.assertEqual(item.count(r'\`') % 2, 0)
        self.assertNotRegex(item, r'backend-\d*…$')

    def test_singular_commit_grammar(self):
        with tempfile.TemporaryDirectory() as repo:
            def git(*args):
                return subprocess.check_output(['git', *args], cwd=repo, text=True).strip()

            git('init', '-q')
            hashes = []
            for subject in ['base', 'single fix']:
                git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                    'commit', '--allow-empty', '-qm', subject)
                hashes.append(git('rev-parse', 'HEAD'))
            tags = Path(repo, 'tags')
            tags.write_text('selfdev-linux-' + hashes[0])
            output = subprocess.check_output(
                [sys.executable, SCRIPT, tags],
                cwd=repo,
                env=dict(os.environ, SHA=hashes[-1], RELEASE_NOTES='Single fix'),
                text=True,
            )
            self.assertIn(f'[View all 1 commit](https://github.com/1jehuang/jcode/compare/{hashes[0]}...{hashes[-1]})', output)

    def test_public_history_and_explicit_override(self):
        with tempfile.TemporaryDirectory() as repo:
            def git(*args):
                return subprocess.check_output(['git', *args], cwd=repo, text=True).strip()

            git('init', '-q')
            hashes = []
            for subject in ['old', 'new', 'newest']:
                git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                    'commit', '--allow-empty', '-qm', subject, '-m', 'private commit body')
                hashes.append(git('rev-parse', 'HEAD'))
            tags = Path(repo, 'tags')

            def render(releases='', notes=''):
                tags.write_text(releases)
                output = subprocess.check_output([sys.executable, SCRIPT, tags], cwd=repo,
                    env=dict(os.environ, SHA=hashes[-1], RELEASE_NOTES=notes), text=True)
                return output.split('## Highlights\n\n')[1].split('\n\n## Bundled changes')[0].splitlines()

            self.assertEqual(render(), ['- newest'])
            self.assertEqual(render('selfdev-linux-' + hashes[0]), ['- newest', '- new'])
            self.assertEqual(render(notes='  - Better picker\nFaster startup'),
                             ['- Better picker', '- Faster startup'])
            tags.write_text('selfdev-linux-' + hashes[0])
            output = subprocess.check_output([sys.executable, SCRIPT, tags], cwd=repo,
                env=dict(os.environ, SHA=hashes[-1], RELEASE_NOTES='Better picker'), text=True)
            self.assertIn(f'[View all 2 commits](https://github.com/1jehuang/jcode/compare/{hashes[0]}...{hashes[-1]})', output)
            self.assertIn('- newest\n- new\n', output)
            self.assertNotIn('private commit body', output)

    def test_bundled_changes_lists_beyond_eight_and_reports_overflow(self):
        with tempfile.TemporaryDirectory() as repo:
            def git(*args):
                return subprocess.check_output(['git', *args], cwd=repo, text=True).strip()

            git('init', '-q')
            hashes = []
            for idx in range(56):
                git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                    'commit', '--allow-empty', '-qm', f'commit-{idx:02d}')
                hashes.append(git('rev-parse', 'HEAD'))
            tags = Path(repo, 'tags')
            tags.write_text('selfdev-linux-' + hashes[0])

            authored = subprocess.check_output(
                [sys.executable, SCRIPT, tags],
                cwd=repo,
                env=dict(os.environ, SHA=hashes[-1], RELEASE_NOTES='Narrow highlight'),
                text=True,
            )
            self.assertIn('[View all 55 commits]', authored)
            self.assertIn('- commit-55\n', authored)
            self.assertIn('- commit-06\n', authored)
            self.assertNotIn('- commit-05\n', authored)
            self.assertIn('- … and 5 more commits in the comparison above.\n', authored)

            fallback = subprocess.check_output(
                [sys.executable, SCRIPT, tags],
                cwd=repo,
                env=dict(os.environ, SHA=hashes[-1], RELEASE_NOTES=''),
                text=True,
            )
            highlights_section, bundled_section = fallback.split('## Bundled changes\n\n')
            self.assertIn('- commit-55\n', highlights_section)
            self.assertIn('- commit-48\n', highlights_section)
            self.assertNotIn('- commit-47\n', highlights_section)
            self.assertIn('- commit-47\n', bundled_section)
            self.assertIn('- commit-06\n', bundled_section)
            self.assertIn('- … and 5 more commits in the comparison above.\n', bundled_section)

    def test_no_user_visible_changes_placeholder_does_not_hide_multi_commit_release(self):
        with tempfile.TemporaryDirectory() as repo:
            def git(*args):
                return subprocess.check_output(['git', *args], cwd=repo, text=True).strip()

            git('init', '-q')
            hashes = []
            for subject in ['base', 'feature: add big capability', 'ci: tweak workflow']:
                git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                    'commit', '--allow-empty', '-qm', subject)
                hashes.append(git('rev-parse', 'HEAD'))
            tags = Path(repo, 'tags')

            # Multi-commit release: "No user-visible changes." falls back to commit subjects.
            tags.write_text('selfdev-linux-' + hashes[0])
            multi = subprocess.check_output(
                [sys.executable, SCRIPT, tags],
                cwd=repo,
                env=dict(os.environ, SHA=hashes[-1], RELEASE_NOTES='No user-visible changes.'),
                text=True,
            )
            highlights = multi.split('## Highlights\n\n')[1].split('\n\n## Bundled changes')[0]
            self.assertIn('feature: add big capability', highlights)
            self.assertIn('ci: tweak workflow', highlights)

            # Single-commit release: "No user-visible changes." is preserved.
            tags.write_text('selfdev-linux-' + hashes[1])
            single = subprocess.check_output(
                [sys.executable, SCRIPT, tags],
                cwd=repo,
                env=dict(os.environ, SHA=hashes[-1], RELEASE_NOTES='No user-visible changes.'),
                text=True,
            )
            single_highlights = single.split('## Highlights\n\n')[1].split('\n\n## Bundled changes')[0]
            self.assertEqual(single_highlights.strip(), '- No user-visible changes.')


if __name__ == '__main__':
    unittest.main()
