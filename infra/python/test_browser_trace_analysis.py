import io
import json
import unittest
from contextlib import redirect_stdout
from dataclasses import asdict

from browser_trace_analysis import TraceError, analyze_trace, print_summary, renderer_for_url, upload_telemetry_mode
from test_browser_upload_analysis import mark


def event(name, pid, tid, *, duration=None, timestamp=0, args=None, phase='X'):
    value = {'name': name, 'pid': pid, 'tid': tid, 'ph': phase, 'ts': timestamp, 'args': args or {}}
    if duration is not None:
        value['dur'] = duration
    return value


class BrowserTraceAnalysisTest(unittest.TestCase):
    def test_disabled_upload_telemetry_is_explicit_and_does_not_remove_renderer_timings(self):
        self.events.append(
            event(
                'garmin.map.upload-telemetry',
                30,
                7,
                phase='I',
                args={'detail': json.dumps(json.dumps({'version': 1, 'enabled': False}))},
            )
        )
        self.events.append(event('garmin.map.wgpu-prepare', 30, 7, duration=300))
        summary = analyze_trace(self.events)
        self.assertIs(summary.upload_telemetry, False)
        self.assertEqual(summary.user_timings['garmin.map.wgpu-prepare'].count, 1)
        self.assertFalse(any('No correlated upload' in warning for warning in summary.diagnostics))
        output = io.StringIO()
        with redirect_stdout(output):
            print_summary(summary)
        self.assertIn('upload telemetry off', output.getvalue())
        self.events.append({**mark('queued', 0), 'pid': 30, 'tid': 7})
        with self.assertRaisesRegex(TraceError, 'despite disabled'):
            analyze_trace(self.events)

    def test_telemetry_configuration_rejects_ambiguity_and_invalid_types(self):
        self.assertIsNone(upload_telemetry_mode([]))
        valid = event(
            'garmin.map.upload-telemetry', 30, 7, phase='I', args={'data': {'detail': {'version': 1, 'enabled': True}}}
        )
        self.assertIs(upload_telemetry_mode([valid]), True)
        with self.assertRaisesRegex(TraceError, 'one startup'):
            upload_telemetry_mode([valid, valid])
        for detail in (
            {'version': True, 'enabled': False},
            {'version': 1, 'enabled': 0},
            {'version': 2, 'enabled': True},
            'broken JSON',
        ):
            with self.subTest(detail=detail), self.assertRaises(TraceError):
                upload_telemetry_mode([{**valid, 'args': {'detail': detail}}])

    def setUp(self):
        self.events = [
            event('thread_name', 20, 1, phase='M', args={'name': 'CrRendererMain'}),
            event('thread_name', 30, 7, phase='M', args={'name': 'CrRendererMain'}),
            event(
                'TracingStartedInBrowser',
                10,
                10,
                phase='I',
                args={
                    'data': {
                        'frames': [
                            {
                                'processId': 20,
                                'url': 'https://duckduckgo.com/chrome_newtab',
                                'isOutermostMainFrame': True,
                            },
                            {
                                'processId': 30,
                                'url': 'http://127.0.0.1:8099/',
                                'isOutermostMainFrame': True,
                            },
                        ]
                    }
                },
            ),
            event('BeginMainThreadFrame', 30, 7, timestamp=0),
            event('BeginMainThreadFrame', 30, 7, timestamp=16_000),
            event('BeginMainThreadFrame', 30, 7, timestamp=51_000),
            event(
                'EventDispatch',
                30,
                7,
                duration=80,
                timestamp=0,
                args={'data': {'type': 'pointerdown'}},
            ),
            event(
                'EventDispatch',
                30,
                7,
                duration=80,
                timestamp=51_000,
                args={'data': {'type': 'pointerup'}},
            ),
            event('RunMicrotasks', 30, 7, duration=40_000),
            event('garmin.map.tile-admission', 30, 7, duration=1_250),
            event(
                'EventDispatch',
                30,
                7,
                duration=80,
                args={'data': {'type': 'wheel'}},
            ),
            event(
                'ResourceSendRequest',
                30,
                7,
                phase='I',
                args={'data': {'url': 'http://127.0.0.1:8099/map/tiles/1/0/0.pbf'}},
            ),
        ]

    def test_selects_renderer_by_page_url_not_an_unrelated_trace_frame(self):
        self.assertEqual(renderer_for_url(self.events, 'http://127.0.0.1:8099/'), (30, 'http://127.0.0.1:8099/'))

    def test_correlated_upload_report_uses_only_selected_main_thread(self):
        lifecycle = [
            mark('queued', 0),
            mark('first_work', 1),
            mark('progress', 2, size=12),
            mark('published', 20),
            mark('first_draw', 30),
        ]
        self.events.extend(dict(item, pid=30, tid=7) for item in lifecycle)
        self.events.extend(dict(item, pid=20, tid=1) for item in lifecycle)
        self.events.extend(dict(item, pid=30, tid=42) for item in lifecycle)
        summary = analyze_trace(self.events)
        self.assertEqual(len(summary.uploads.uploads), 1)
        self.assertEqual(summary.uploads.uploads[0].published_ms, 20)
        encoded = json.loads(json.dumps(asdict(summary)))
        self.assertEqual(encoded['uploads']['uploads'][0]['first_draw_ms'], 30)
        output = io.StringIO()
        with redirect_stdout(output):
            print_summary(summary)
        self.assertIn('publication to first draw', output.getvalue())
        self.assertIn('offscreen retention', output.getvalue())

    def test_old_traces_explicitly_report_missing_upload_correlation(self):
        self.assertTrue(any('No correlated upload' in message for message in analyze_trace(self.events).diagnostics))

    def test_bad_upload_payload_and_timestamp_do_not_enter_report_percentiles(self):
        for bad in (mark('progress', 3, work=-1), dict(mark('progress', 3, size=12), ts='3000')):
            with self.subTest(bad=bad):
                lifecycle = [
                    mark('queued', 0),
                    mark('first_work', 1),
                    mark('progress', 2, size=12),
                    bad,
                    mark('published', 10),
                    mark('first_draw', 20),
                ]
                events = self.events + [dict(item, pid=30, tid=7) for item in lifecycle]
                summary = analyze_trace(events)
                self.assertEqual(summary.uploads.uploads[0].status, 'invalid')
                output = io.StringIO()
                with redirect_stdout(output):
                    print_summary(summary)
                self.assertIn('visible queue lifetime     none', output.getvalue())
                self.assertTrue(any('lifecycle gaps' in warning for warning in summary.diagnostics))

    def test_summarizes_frames_stalls_input_tiles_and_user_timing(self):
        summary = analyze_trace(self.events)

        self.assertEqual(summary.renderer_pid, 30)
        self.assertEqual(summary.renderer_tid, 7)
        frames = summary.frame_intervals
        interaction_frames = summary.interaction_frame_intervals
        microtasks = summary.blocking_microtasks
        if frames is None or interaction_frames is None or microtasks is None:
            self.fail('synthetic trace did not produce expected distributions')
        self.assertEqual(frames.count, 2)
        self.assertEqual(summary.frame_stalls, 1)
        self.assertEqual(interaction_frames.count, 2)
        self.assertEqual(summary.interaction_frame_stalls, 1)
        self.assertEqual(microtasks.maximum, 40.0)
        self.assertEqual(summary.event_dispatch['wheel'].maximum, 0.08)
        self.assertEqual(summary.user_timings['garmin.map.tile-admission'].maximum, 1.25)
        self.assertEqual(summary.tile_requests, 1)

    def test_rejects_a_trace_without_the_requested_page(self):
        with self.assertRaises(TraceError):
            analyze_trace(self.events, 'https://missing.example/')

    def test_data_admission_is_not_dropped_or_counted_as_tile_work(self):
        events = [item for item in self.events if item['name'] != 'garmin.map.tile-admission']
        events.append(event('garmin.map.data-admission', 30, 7, duration=50_000))
        summary = analyze_trace(events)
        self.assertEqual(summary.user_timings['garmin.map.data-admission'].maximum, 50.0)
        self.assertNotIn('garmin.map.tile-admission', summary.user_timings)

    def test_follows_navigation_from_blank_page(self):
        self.events[2]['args']['data']['frames'][1]['url'] = 'about:blank'
        self.events.append(
            event(
                'FrameCommittedInBrowser',
                10,
                10,
                phase='I',
                args={
                    'data': {
                        'processId': 30,
                        'url': 'http://127.0.0.1:8099/',
                        'isOutermostMainFrame': True,
                    }
                },
            )
        )
        self.assertEqual(renderer_for_url(self.events, 'http://127.0.0.1:8099/')[0], 30)

    def test_rejects_ambiguous_renderer_after_navigation(self):
        self.events.append(
            event(
                'FrameCommittedInBrowser',
                10,
                10,
                phase='I',
                args={
                    'data': {
                        'processId': 40,
                        'url': 'http://127.0.0.1:8099/',
                        'isOutermostMainFrame': True,
                    }
                },
            )
        )
        with self.assertRaises(TraceError):
            renderer_for_url(self.events, 'http://127.0.0.1:8099/')

    def test_reads_paired_and_zero_duration_measures_without_unmatched_spans(self):
        for phase, timestamp, identity in [
            ('b', 100, 'a'),
            ('b', 200, 'b'),
            ('e', 600, 'b'),
            ('e', 1_100, 'a'),
            ('b', 1_200, 'c'),
            ('e', 1_300, 'd'),
        ]:
            measure = event('garmin.map.wgpu-prepare', 30, 7, phase=phase, timestamp=timestamp)
            measure['id2'] = {'local': identity}
            self.events.append(measure)
        self.events.append(event('garmin.map.wgpu-prepare', 30, 7, phase='n', timestamp=1_500))
        timing = analyze_trace(self.events).user_timings['garmin.map.wgpu-prepare']
        self.assertEqual(timing.count, 3)
        self.assertEqual(timing.maximum, 1.0)
        self.assertEqual(timing.p50, 0.4)

    def test_exposes_fallback_and_separates_worker_requests(self):
        self.events.append(event('garmin.map.worker-fallback', 30, 7, phase='n'))
        self.events.append(
            event(
                'ResourceSendRequest',
                30,
                8,
                phase='I',
                args={
                    'data': {
                        'url': 'http://127.0.0.1:8099/map/tiles/1/1/1.pbf',
                    }
                },
            )
        )
        summary = analyze_trace(self.events)
        self.assertEqual(summary.tile_requests, 1)
        self.assertEqual(summary.worker_tile_requests, 1)
        self.assertTrue(any('fallback was used' in message for message in summary.diagnostics))

    def test_missing_admission_is_not_reported_as_success(self):
        self.events = [item for item in self.events if item['name'] != 'garmin.map.tile-admission']
        self.assertTrue(any('unverified' in message for message in analyze_trace(self.events).diagnostics))

    def test_allocation_measurements_are_distinct_from_copy_and_decode(self):
        self.assertTrue(
            any('allocation cost is unverified' in message for message in analyze_trace(self.events).diagnostics)
        )
        self.events.append(event('garmin.map.tile-allocation', 30, 7, duration=2_500))
        summary = analyze_trace(self.events)
        self.assertEqual(summary.user_timings['garmin.map.tile-allocation'].maximum, 2.5)
        self.assertFalse(any('allocation cost is unverified' in message for message in summary.diagnostics))

    def interaction_summary(self, frames, inputs):
        events = self.events[:3]
        events += [event('BeginMainThreadFrame', 30, 7, timestamp=time * 1_000) for time in frames]
        events += [
            event(
                'EventDispatch', 30, 7, timestamp=time * 1_000, duration=duration * 1_000, args={'data': {'type': kind}}
            )
            for kind, time, duration in inputs
        ]
        return analyze_trace(events)

    def test_keeps_intervals_crossing_both_gesture_boundaries(self):
        summary = self.interaction_summary([-16, 200, 216, 250], [('pointerdown', 0, 200), ('pointerup', 225, 0)])
        self.assertEqual(summary.interaction_frame_intervals.maximum, 216)
        self.assertEqual(summary.interaction_frame_intervals.count, 3)
        self.assertEqual(summary.interaction_frame_stalls, 2)

    def test_excludes_idle_frames_between_separate_gestures(self):
        summary = self.interaction_summary(
            list(range(0, 1_101, 10)),
            [('pointerdown', 15, 0), ('pointerup', 25, 0), ('pointerdown', 1_015, 0), ('pointerup', 1_025, 0)],
        )
        self.assertEqual(summary.interaction_frame_intervals.count, 4)

    def test_single_wheel_event_includes_response_and_tail(self):
        summary = self.interaction_summary([-16, 100, 216, 232], [('wheel', 0, 0)])
        self.assertEqual(summary.interaction_frame_intervals.count, 2)
        self.assertEqual(summary.interaction_frame_intervals.maximum, 116)

    def test_overlapping_gestures_do_not_duplicate_intervals(self):
        summary = self.interaction_summary([0, 16, 32], [('wheel', 1, 0), ('wheel', 2, 0)])
        self.assertEqual(summary.interaction_frame_intervals.count, 2)

    def resource(self, name, identity, timestamp, *, pid=30, tid=8, **data):
        return event(name, pid, tid, phase='I', timestamp=timestamp, args={'data': {'requestId': identity, **data}})

    def test_loading_matches_requests_across_threads_and_ignores_other_renderers(self):
        url = 'http://127.0.0.1:8099/map/tiles/11/1023/680.pbf'
        self.events += [
            self.resource('ResourceFinish', 'tile', 15_000, tid=9, didFail=False),
            self.resource('ResourceReceiveResponse', 'tile', 12_000, statusCode=200, fromCache=True),
            self.resource('ResourceSendRequest', 'tile', 10_000, url=url),
            self.resource('ResourceSendRequest', 'tile', 0, pid=20, url=url),
            self.resource('ResourceFinish', 'tile', 999_000, pid=20, didFail=True),
            self.resource('ResourceSendRequest', 'other', 0, url='http://127.0.0.1:8099/avatar'),
        ]
        loading = analyze_trace(self.events).tile_loading
        assert loading.request_latency is not None
        self.assertEqual(loading.request_latency.maximum, 5)
        self.assertEqual(loading.cached, 1)
        self.assertEqual(loading.statuses, {'200': 1})
        self.assertEqual(loading.failed, 0)
        self.assertEqual(loading.requests[0].path, '/map/tiles/11/1023/680.pbf')
        self.assertEqual(loading.requests[0].started_ms, 0)

    def test_loading_keeps_failed_unfinished_and_unknown_cache_distinct(self):
        url = 'http://127.0.0.1:8099/map/tiles/1/0/0.pbf'
        self.events += [
            self.resource('ResourceSendRequest', identity, 1_000, url=url)
            for identity in ['failed', 'unfinished', 'http-error']
        ]
        self.events += [
            self.resource('ResourceFinish', 'failed', 2_000, didFail=True),
            self.resource('ResourceReceiveResponse', 'http-error', 3_000, statusCode=503, fromCache=False),
            self.resource('ResourceFinish', 'http-error', 4_000, didFail=False),
            self.resource('ResourceFinish', 'not-captured', 999_000, didFail=False),
        ]
        loading = analyze_trace(self.events).tile_loading
        self.assertEqual((loading.failed, loading.unfinished, loading.cache_unknown, loading.uncached), (1, 1, 2, 1))
        self.assertEqual(loading.statuses, {'503': 1})
        assert loading.request_latency is not None
        self.assertEqual(loading.request_latency.count, 1)
        self.assertEqual(loading.request_latency.maximum, 3)

    def test_redirect_preserves_original_request_start(self):
        self.events += [
            self.resource('ResourceSendRequest', 'redirect', 1_000, url='http://host/map/tiles/1/0/0.pbf'),
            self.resource('ResourceSendRequest', 'redirect', 2_000, url='https://host/map/tiles/1/0/0.pbf'),
            self.resource('ResourceFinish', 'redirect', 5_000, didFail=False),
        ]
        loading = analyze_trace(self.events).tile_loading
        self.assertEqual(len(loading.requests), 1)
        assert loading.request_latency is not None
        self.assertEqual(loading.request_latency.maximum, 4)

    def test_timeline_splits_worker_tasks_and_does_not_double_count_nested_spans(self):
        self.events += [
            self.resource('ResourceSendRequest', 'tile', 1_000_000, url='http://host/map/tiles/1/0/0.pbf'),
            event('RunTask', 30, 8, timestamp=1_900_000, duration=200_000),
            event('RunTask', 30, 8, timestamp=1_950_000, duration=50_000),
            event('FunctionCall', 30, 8, timestamp=1_900_000, duration=200_000),
            event('RunTask', 30, 7, timestamp=1_900_000, duration=300_000),
            event('RunTask', 20, 8, timestamp=1_900_000, duration=400_000),
            event('SchedulePostMessage', 30, 8, phase='I', timestamp=2_050_000),
            event('SchedulePostMessage', 30, 7, phase='I', timestamp=2_050_000),
            event('garmin.map.tile-admission', 30, 7, phase='b', timestamp=2_100_000),
            event('garmin.map.tile-admission', 30, 7, phase='e', timestamp=2_101_000),
            event('garmin.map.tile-allocation', 30, 7, phase='n', timestamp=2_100_000),
        ]
        loading = analyze_trace(self.events).tile_loading
        first, second = loading.timeline
        self.assertEqual(loading.worker_busy_ms, 200)
        self.assertEqual((first.worker_busy_ms, second.worker_busy_ms), (100, 100))
        self.assertEqual(
            (first.requests, second.worker_replies, second.allocation_calls, second.admission_frames), (1, 1, 1, 1)
        )

    def test_empty_loading_data_is_explicit_and_json_serializable(self):
        summary = analyze_trace(self.events[:3])
        loading = json.loads(json.dumps(asdict(summary)))['tile_loading']
        self.assertIsNone(loading['request_latency'])
        self.assertIsNone(loading['worker_tasks'])
        self.assertEqual(loading['timeline'], [])
        output = io.StringIO()
        with redirect_stdout(output):
            print_summary(summary, timeline=True)
        self.assertIn('tile latency none', output.getvalue())
        self.assertIn('end-to-end visibility requires correlated tile markers', output.getvalue())

    def test_lifecycle_latency_is_separate_from_frame_work(self):
        self.events += [
            event('garmin.map.tile-admission-wait', 30, 7, duration=120_000),
            event('garmin.map.tile-admission-latency', 30, 7, duration=140_000),
            event('garmin.map.tile-upload-latency', 30, 7, duration=80_000),
        ]
        summary = analyze_trace(self.events)
        self.assertEqual(summary.user_timings['garmin.map.tile-admission-wait'].maximum, 120)
        self.assertEqual(summary.user_timings['garmin.map.tile-admission-latency'].maximum, 140)
        self.assertEqual(summary.user_timings['garmin.map.tile-upload-latency'].maximum, 80)
        self.assertEqual(summary.user_timings['garmin.map.tile-admission'].maximum, 1.25)
        self.assertFalse(any('lifecycle markers are missing' in message for message in summary.diagnostics))


if __name__ == '__main__':
    unittest.main()
