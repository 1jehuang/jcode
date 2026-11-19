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
        result = bullets('- <script> @everyone\n* `example`\u202e\n' + ('z' * 300 + '\n') * 20)
        self.assertEqual(result[:2], ['- &lt;script&gt; ＠everyone', r'- \`example\`'])
        self.assertEqual(len(result), 8)
        self.assertEqual(len(result[-1]), 202)

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


if __name__ == '__main__':
    unittest.main()
