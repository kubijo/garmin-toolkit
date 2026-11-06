"""Validate the gh serialization boundary independently of publication logic."""

import json
import unittest
from pathlib import Path
from unittest.mock import Mock

from github_releases import GitHubReleases, ReleaseAsset


class GitHubReleaseTests(unittest.TestCase):
    def test_lightweight_and_annotated_tags_resolve_to_the_commit(self) -> None:
        commit = 'a' * 40
        tag_object = 'b' * 40
        for references in [
            f'{commit}\trefs/tags/v1.2.3\n',
            f'{tag_object}\trefs/tags/v1.2.3\n{commit}\trefs/tags/v1.2.3^{{}}\n',
        ]:
            with self.subTest(references=references):
                client = GitHubReleases('owner/repository', Mock(return_value=references))
                self.assertEqual(client.tag_commit('v1.2.3'), commit)

    def test_deleted_or_unexpected_remote_tag_is_rejected(self) -> None:
        for references in ['', f'{"a" * 40}\trefs/heads/main\n']:
            with self.subTest(references=references):
                client = GitHubReleases('owner/repository', Mock(return_value=references))
                with self.assertRaises(ValueError):
                    client.tag_commit('v1.2.3')

    def test_draft_state_must_be_a_json_boolean(self) -> None:
        for draft in [True, False, 'false', None]:
            with self.subTest(draft=draft):
                client = GitHubReleases('owner/repository', Mock(return_value=json.dumps({'isDraft': draft})))
                if isinstance(draft, bool):
                    self.assertEqual(client.is_draft('v1.2.3'), draft)
                else:
                    with self.assertRaises(ValueError):
                        client.is_draft('v1.2.3')

    def test_asset_metadata_is_typed_and_duplicate_names_are_rejected(self) -> None:
        rows = [{'name': 'one.AppImage', 'digest': 'sha256:synthetic'}, {'name': 'two.AppImage', 'digest': None}]
        run = Mock(return_value=json.dumps({'assets': rows}))
        client = GitHubReleases('owner/repository', run)
        self.assertEqual(
            client.assets('v1.2.3'),
            [ReleaseAsset('one.AppImage', 'sha256:synthetic'), ReleaseAsset('two.AppImage', None)],
        )
        run.return_value = json.dumps({'assets': [*rows, rows[0]]})
        with self.assertRaisesRegex(ValueError, 'duplicate'):
            client.assets('v1.2.3')

    def test_download_pattern_and_destination_are_distinct_arguments(self) -> None:
        run = Mock(return_value='')
        client = GitHubReleases('owner/repository', run)
        client.download('v1.2.3', Path('/tmp/path with spaces'), pattern='one.AppImage')
        run.assert_called_once_with(
            'gh',
            'release',
            'download',
            'v1.2.3',
            '--repo',
            'owner/repository',
            '--dir',
            '/tmp/path with spaces',
            '--pattern',
            'one.AppImage',
        )

    def test_upload_never_requests_asset_replacement(self) -> None:
        run = Mock(return_value='')
        client = GitHubReleases('owner/repository', run)
        client.upload('v1.2.3', [Path('/tmp/one.AppImage')])
        self.assertNotIn('--clobber', run.call_args.args)
        self.assertIn('/tmp/one.AppImage', run.call_args.args)


if __name__ == '__main__':
    unittest.main()
