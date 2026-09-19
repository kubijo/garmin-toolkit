import json
import unittest

from browser_upload_analysis import analyze_uploads


def mark(phase, elapsed, *, identity=1, timestamp=None, work=0, size=0, encoding=0):
    detail = {
        'version': 1,
        'upload_id': identity,
        'zoom': 3,
        'x': 4,
        'y': 2,
        'event': phase,
        'elapsed_ms': elapsed,
        'work_ms': work,
        'bytes': size,
    }
    for _ in range(encoding):
        detail = json.dumps(detail)
    return {
        'name': 'garmin.map.upload',
        'ts': (elapsed if timestamp is None else timestamp) * 1_000,
        'ph': 'I',
        'args': {'detail': detail},
    }


class UploadAnalysisTest(unittest.TestCase):
    def test_stationary_loading_and_first_draw_are_separate(self):
        result = analyze_uploads(
            [
                mark('queued', 0),
                mark('first_work', 10),
                mark('progress', 12, work=2, size=128),
                mark('published', 30),
                mark('first_draw', 47),
            ]
        )
        self.assertEqual(result.malformed_events, 0)
        row = result.uploads[0]
        self.assertEqual(row.status, 'drawn')
        self.assertEqual((row.visible_ms, row.hidden_ms, row.work_ms, row.bytes), (30, 0, 2, 128))
        self.assertEqual(row.first_work_ms, 10)
        self.assertEqual(row.visible_wait_ms, 28)
        self.assertEqual(row.last_visible_to_publish_ms, 30)
        self.assertEqual(row.publication_to_draw_ms, 17)

    def test_pan_away_and_back_does_not_count_retention_as_visible_wait(self):
        result = analyze_uploads(
            [
                mark('queued', 0),
                mark('first_work', 1),
                mark('progress', 2, work=0.5, size=256),
                mark('hidden', 10),
                mark('visible', 2_610),
                mark('progress', 2_612, work=1, size=512),
                mark('hidden', 2_615),
                mark('visible', 2_645),
                mark('published', 2_650),
                mark('first_draw', 2_666),
            ]
        )
        row = result.uploads[0]
        self.assertEqual(row.visible_ms, 20)
        self.assertEqual(row.hidden_ms, 2_630)
        self.assertEqual(row.published_ms, row.visible_ms + row.hidden_ms)
        self.assertEqual(row.last_visible_to_publish_ms, 5)
        self.assertEqual(row.work_ms, 1.5)
        self.assertEqual(row.bytes, 768)
        self.assertEqual(row.visibility_changes, 4)

    def test_chrome_detail_string_encodings(self):
        for encoding in range(3):
            with self.subTest(encoding=encoding):
                result = analyze_uploads([mark('queued', 0, encoding=encoding), mark('released', 5, encoding=encoding)])
                self.assertEqual(result.uploads[0].status, 'released')
                self.assertEqual(result.malformed_events, 0)

    def test_eviction_and_truncated_captures_are_not_completed_uploads(self):
        result = analyze_uploads(
            [
                mark('progress', 5, identity=9),
                mark('queued', 0),
                mark('hidden', 10),
                mark('released', 50),
                mark('queued', 0, identity=2, timestamp=60),
            ]
        )
        self.assertEqual(result.partial_events, 1)
        self.assertEqual([row.status for row in result.uploads], ['released', 'incomplete'])
        self.assertEqual(result.uploads[0].hidden_ms, 40)
        self.assertIsNone(result.uploads[1].published_ms)

    def test_reused_ids_after_navigation_do_not_merge_uploads(self):
        result = analyze_uploads(
            [
                mark('queued', 0),
                mark('released', 1),
                mark('queued', 0, timestamp=100),
                mark('first_work', 2, timestamp=102),
                mark('progress', 2, timestamp=102, size=12),
                mark('published', 3, timestamp=103),
            ]
        )
        self.assertEqual(len(result.uploads), 2)
        self.assertEqual([row.status for row in result.uploads], ['released', 'published'])
        self.assertEqual(result.uploads[1].visible_ms, 3)

    def test_malformed_and_out_of_order_events_are_not_success(self):
        sequences = [
            [mark('queued', 0), mark('first_draw', 5)],
            [mark('queued', 0), mark('hidden', 5), mark('first_work', 6)],
            [mark('queued', 0), mark('hidden', 5), mark('hidden', 6)],
            [mark('queued', 0), mark('first_work', 10), mark('published', 5, timestamp=15)],
        ]
        for events in sequences:
            with self.subTest(events=events):
                result = analyze_uploads(events)
                self.assertEqual(result.malformed_events, 1)
                self.assertEqual(result.uploads[0].status, 'invalid')
        for value in [float('nan'), float('inf'), -1, True]:
            with self.subTest(value=value):
                result = analyze_uploads([mark('queued', value, timestamp=0)])
                self.assertEqual(result.malformed_events, 1)
                self.assertEqual(result.uploads, [])

    def test_bad_middle_payload_invalidates_only_its_identifiable_upload(self):
        for encoding in range(3):
            result = analyze_uploads(
                [
                    mark('queued', 0),
                    mark('first_work', 1),
                    mark('progress', 1, size=12),
                    mark('progress', 2, work=-1, size=256, encoding=encoding),
                    mark('published', 10),
                    mark('first_draw', 20),
                    mark('queued', 0, identity=2),
                    mark('first_work', 1, identity=2),
                    mark('progress', 2, size=12, identity=2),
                    mark('published', 10, identity=2),
                    mark('first_draw', 20, identity=2),
                ]
            )
            self.assertEqual(result.malformed_events, 1)
            self.assertEqual([row.status for row in result.uploads], ['invalid', 'drawn'])

    def test_missing_progress_is_invalid_but_zero_duration_work_is_valid(self):
        for progress, expected in (
            ([], 'invalid'),
            ([mark('progress', 2)], 'invalid'),
            ([mark('progress', 2, size=12)], 'drawn'),
        ):
            with self.subTest(progress=progress):
                result = analyze_uploads(
                    [
                        mark('queued', 0),
                        mark('first_work', 1),
                        *progress,
                        mark('published', 10),
                        mark('first_draw', 20),
                    ]
                )
                self.assertEqual(result.uploads[0].status, expected)

    def test_unidentifiable_gap_invalidates_the_cohort(self):
        result = analyze_uploads(
            [
                mark('queued', 0),
                mark('first_work', 1),
                mark('progress', 2, size=12),
                {'name': 'garmin.map.upload', 'ts': 3_000, 'args': {'detail': 'broken JSON'}},
                mark('published', 10),
                mark('first_draw', 20),
            ]
        )
        self.assertEqual(result.malformed_events, 1)
        self.assertEqual(result.uploads[0].status, 'invalid')

    def test_invalid_timestamps_never_sort_or_support_completion(self):
        for timestamp in (None, '1000', True, float('nan'), float('inf'), 10**400):
            with self.subTest(timestamp=timestamp):
                bad = mark('progress', 2, size=12)
                if timestamp is None:
                    del bad['ts']
                else:
                    bad['ts'] = timestamp
                result = analyze_uploads(
                    [
                        mark('queued', 0),
                        mark('first_work', 1),
                        bad,
                        mark('progress', 3, size=12),
                        mark('published', 10),
                        mark('first_draw', 20),
                    ]
                )
                self.assertEqual(result.malformed_events, 1)
                self.assertEqual(result.uploads[0].status, 'invalid')
                self.assertIsNotNone(result.uploads[0].error)


if __name__ == '__main__':
    unittest.main()
