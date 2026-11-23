import io
import json
import unittest
from unittest.mock import patch

import refresh_selfdev_discord as refresh


class RefreshTest(unittest.TestCase):
    def setUp(self):
        tag = 'selfdev-linux-' + 'a' * 40
        self.mid = '123456789012345678'
        self.args = dict(tag=tag, message_id=self.mid, repository='1jehuang/jcode',
                         token='test', webhook_url='https://discord.invalid/api/webhooks/1/x')
        self.url = 'https://github.com/1jehuang/jcode/releases/tag/' + tag
        self.release = dict(tag_name=tag, html_url=self.url, prerelease=True, draft=False,
                            body=refresh.announcements.announcement_marker(tag))
        self.message = dict(id=self.mid, content='Download: ' + self.url)
        self.fetch = self.mock(refresh.announcements, 'fetch_release')
        self.fetch.return_value = self.release
        self.http = self.mock(refresh.urllib.request, 'urlopen')
        self.http.side_effect = lambda *a, **kw: io.BytesIO(json.dumps(self.message).encode())

    def mock(self, obj, attr):
        p = patch.object(obj, attr)
        self.addCleanup(p.stop)
        return p.start()

    def methods(self):
        return [call.args[0].method for call in self.http.call_args_list]

    def test_refresh(self):
        refresh.refresh(**self.args)
        self.assertEqual(self.methods(), ['GET', 'PATCH'])
        req = self.http.call_args.args[0]
        self.assertEqual(req.full_url, self.args['webhook_url'] + '/messages/' + self.mid)
        self.assertEqual(json.loads(req.data), dict(content=refresh.announcements.format_message(
            tag=self.args['tag'], name=self.args['tag'], body=self.release['body'], url=self.url),
            flags=4, allowed_mentions={'parse': []}))

    def test_invalid_release(self):
        for changes in [dict(draft=True), dict(prerelease=False), dict(body=''),
                        dict(html_url=self.url + 'a'), dict(tag_name='unrelated')]:
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                self.fetch.return_value = {**self.release, **changes}
                refresh.refresh(**self.args)
        self.http.assert_not_called()

    def test_refresh_masked_link(self):
        self.message['content'] = f'Build notes\n[Download & release notes](<{self.url}>)'
        refresh.refresh(**self.args)
        self.assertEqual(self.methods(), ['GET', 'PATCH'])

    def test_invalid_inputs(self):
        for changes in [dict(message_id='12/34'), dict(tag='not-a-tag'), dict(tag='v1.0.0-alpha')]:
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                refresh.refresh(**{**self.args, **changes})
        self.fetch.assert_not_called()

    def test_refresh_stable_release_by_url_and_heading(self):
        tag = 'v1.0.3'
        url = 'https://github.com/1jehuang/jcode/releases/tag/' + tag
        release = dict(
            tag_name=tag,
            name=tag,
            html_url=url,
            prerelease=False,
            draft=False,
            body='### Highlights\n\n- Great feature.\n\n' + refresh.announcements.announcement_marker(tag),
        )
        self.fetch.return_value = release
        stable_args = {**self.args, 'tag': tag}

        # 1. Existing message matching release URL uses single-part override when available.
        self.message['content'] = f'# jcode {tag}\nOld blurb\nDownload: <{url}>'
        with patch.object(refresh.announcements, 'load_announcement', return_value=['# jcode v1.0.3\n\nSynced!']):
            refresh.refresh(**stable_args)
        self.assertEqual(self.methods(), ['GET', 'PATCH'])
        req = self.http.call_args.args[0]
        self.assertEqual(json.loads(req.data)['content'], '# jcode v1.0.3\n\nSynced!')

        # 2. Existing message matching heading falls back to format_message when no override.
        self.http.reset_mock()
        self.message['content'] = f'## {tag}\nOld auto-formatted body'
        with patch.object(refresh.announcements, 'load_announcement', return_value=None):
            refresh.refresh(**stable_args)
        self.assertEqual(self.methods(), ['GET', 'PATCH'])
        req = self.http.call_args.args[0]
        self.assertEqual(
            json.loads(req.data)['content'],
            refresh.announcements.format_message(tag=tag, name=tag, body=release['body'], url=url),
        )

        # 3. Prefix collision (v1.0.30) must be rejected.
        self.http.reset_mock()
        self.message['content'] = '## v1.0.30\nDifferent release'
        with self.assertRaises(ValueError):
            refresh.refresh(**stable_args)
        self.assertEqual(self.methods(), ['GET'])

        # 4. Multi-part override cannot be patched into one message ID.
        self.http.reset_mock()
        self.message['content'] = f'## {tag}\nOld body'
        with (
            patch.object(refresh.announcements, 'load_announcement', return_value=['part 1', 'part 2']),
            self.assertRaises(ValueError),
        ):
            refresh.refresh(**stable_args)
        self.assertEqual(self.methods(), ['GET'])

        # 5. Stable tag with prerelease=True must be rejected.
        self.http.reset_mock()
        self.fetch.return_value = {**release, 'prerelease': True}
        with self.assertRaises(ValueError):
            refresh.refresh(**stable_args)
        self.http.assert_not_called()

    def test_unrelated_messages(self):
        for changes in [dict(content=self.url + 'a'), dict(content=''), dict(id='different'),
                        dict(content=f'[Download & release notes](<{self.url}/other>)')]:
            with self.subTest(changes=changes), patch.dict(self.message, changes):
                self.http.reset_mock()
                with self.assertRaises(ValueError):
                    refresh.refresh(**self.args)
                self.assertEqual(self.methods(), ['GET'])

    def test_failed_edit_never_posts(self):
        self.http.side_effect = [io.BytesIO(json.dumps(self.message).encode()), TimeoutError()]
        with self.assertRaises(TimeoutError):
            refresh.refresh(**self.args)
        self.assertEqual(self.methods(), ['GET', 'PATCH'])


if __name__ == '__main__':
    unittest.main()
