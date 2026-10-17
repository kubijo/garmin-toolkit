import gzip
import json
import tempfile
import unittest
from copy import deepcopy
from dataclasses import replace
from pathlib import Path

from desktop_profile import (
    COMBINED_NAME,
    CURRENT_METRICS_SCHEMA_VERSION,
    MANIFEST_NAME,
    MANIFEST_SCHEMA_VERSION,
    METRICS_NAME,
    file_digest,
)
from desktop_profile_analysis import Distribution, analyze_report, comparison_mismatches, interaction_gate


class DesktopProfileAnalysisTests(unittest.TestCase):
    def complete_manifest(self, report: Path) -> None:
        artifacts = {
            path.name: {'bytes': path.stat().st_size, 'sha256': file_digest(path)}
            for path in report.iterdir()
            if path.is_file() and path.name != MANIFEST_NAME
        }
        (report / MANIFEST_NAME).write_text(
            json.dumps(
                {
                    'schema_version': MANIFEST_SCHEMA_VERSION,
                    'status': 'complete',
                    'mode': 'demo',
                    'artifacts': artifacts,
                    'git': {'dirty': False},
                    'host': {'platform': 'test', 'machine': 'test', 'perf_event_paranoid': '-1'},
                    'tools': {'rustc': 'rustc test', 'samply': 'samply test'},
                    'build': {'command': ['cargo', 'build'], 'environment': {'RUSTFLAGS': 'test'}},
                    'record_command': [
                        'samply',
                        'record',
                        '--output',
                        'profile.json.gz',
                        '--profile-name',
                        'test',
                        '--gfx',
                        '--',
                        '/tmp/garmin-desktop',
                    ],
                }
            )
        )

    def test_distribution_uses_nearest_rank_percentiles(self) -> None:
        distribution = Distribution.from_values([4.0, 1.0, 3.0, 2.0])

        self.assertIsNotNone(distribution)
        assert distribution is not None
        self.assertEqual(distribution.p50, 2.0)
        self.assertEqual(distribution.p95, 4.0)
        self.assertEqual(distribution.p99, 4.0)
        self.assertEqual(distribution.maximum, 4.0)

    def test_interaction_gate_enforces_frame_and_stall_limits(self) -> None:
        passing = Distribution(180, 12.0, 12.0, 16.0, 20.0, 32.0)
        slow = Distribution(180, 12.0, 12.0, 17.0, 20.0, 32.0)
        stalled = Distribution(180, 12.0, 12.0, 16.0, 20.0, 34.0)
        sparse = Distribution(1, 12.0, 12.0, 12.0, 12.0, 12.0)

        self.assertEqual(interaction_gate(passing, passing), 'PASS')
        self.assertEqual(interaction_gate(slow, passing), 'FAIL')
        self.assertEqual(interaction_gate(stalled, passing), 'FAIL')
        self.assertEqual(interaction_gate(passing, slow), 'FAIL')
        self.assertTrue(interaction_gate(sparse, passing).startswith('INSUFFICIENT'))
        self.assertEqual(interaction_gate(None, passing), 'unavailable')
        self.assertEqual(interaction_gate(passing, None), 'unavailable')

    def test_analysis_resolves_symbols_and_isolates_interaction_intervals(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            report = Path(temporary)
            profile = {
                'meta': {'startTime': 1_000.0},
                'libs': [{'codeId': 'app-code'}],
                'threads': [
                    {
                        'name': 'garmin-desktop',
                        'processName': 'garmin-desktop',
                        'isMainThread': True,
                        'pid': 'application',
                        'samples': {
                            'length': 2,
                            'stack': [0, 0],
                            'threadCPUDelta': [1_000.0, 1_000.0],
                        },
                        'stackTable': {'frame': [0], 'prefix': [None]},
                        'frameTable': {'func': [0], 'address': [16]},
                        'funcTable': {'name': [0], 'resource': [0]},
                        'resourceTable': {'lib': [0]},
                        'stringArray': ['0x10'],
                    },
                    {
                        'name': 'samply',
                        'processName': 'samply',
                        'isMainThread': True,
                        'pid': 'profiler',
                        'samples': {'length': 1, 'stack': [None], 'threadCPUDelta': [1_000_000.0]},
                    },
                ],
            }
            with gzip.open(report / COMBINED_NAME, 'wt') as destination:
                json.dump(profile, destination)
            sidecar = {
                'string_table': ['garmin_ui::activity::map::paint', 'core::iter::outer'],
                'data': [
                    {
                        'code_id': 'app-code',
                        'known_addresses': [[16, 0]],
                        'symbol_table': [{'symbol': 1, 'frames': [{'function': 0}, {'function': 1}]}],
                    }
                ],
            }
            (report / 'combined.json.syms.json').write_text(json.dumps(sidecar))
            rows = [
                {
                    'kind': 'header',
                    'schema_version': CURRENT_METRICS_SCHEMA_VERSION,
                    'started_unix_milliseconds': 1_000.0,
                },
                {
                    'kind': 'map_frame',
                    'elapsed_milliseconds': 11.0,
                    'frame_milliseconds': 120.0,
                    'interaction_frame_milliseconds': None,
                    'camera_active': True,
                    'ui_milliseconds': 0.5,
                    'scene_milliseconds': 0.01,
                    'route_query_microseconds': 1.0,
                    'label_milliseconds': 2.0,
                    'label_backlog': 1,
                    'stale_work': 0,
                    'visible_tiles': 3,
                    'ready_tiles': 2,
                    'pending_tiles': 1,
                    'queued_upload_bytes': 0,
                    'uploaded_bytes': 0,
                },
                {
                    'kind': 'desktop_frame',
                    'elapsed_milliseconds': 12.0,
                    'frame_milliseconds': 16.0,
                    'ui_milliseconds': 1.0,
                },
                {
                    'kind': 'map_frame',
                    'elapsed_milliseconds': 27.0,
                    'frame_milliseconds': 16.0,
                    'interaction_frame_milliseconds': 16.0,
                    'camera_active': True,
                    'ui_milliseconds': 0.6,
                    'scene_milliseconds': 0.02,
                    'route_query_microseconds': 2.0,
                    'label_milliseconds': 2.0,
                    'label_backlog': 0,
                    'stale_work': 0,
                    'visible_tiles': 3,
                    'ready_tiles': 3,
                    'pending_tiles': 0,
                    'queued_upload_bytes': 0,
                    'uploaded_bytes': 0,
                },
                {
                    'kind': 'map_render',
                    'elapsed_milliseconds': 28.0,
                    'phase': 'prepare',
                    'milliseconds': 0.01,
                },
                {
                    'kind': 'desktop_frame',
                    'elapsed_milliseconds': 29.0,
                    'frame_milliseconds': 16.0,
                    'ui_milliseconds': 1.1,
                },
                {
                    'kind': 'map_render',
                    'elapsed_milliseconds': 30.0,
                    'phase': 'prepare',
                    'milliseconds': 0.01,
                },
                {
                    'kind': 'map_render',
                    'elapsed_milliseconds': 31.0,
                    'phase': 'draw',
                    'milliseconds': 0.02,
                },
            ]
            (report / METRICS_NAME).write_text(''.join(f'{json.dumps(row)}\n' for row in rows))
            self.complete_manifest(report)

            analysis = analyze_report(report)

            interaction_frame = analysis.runtime.interaction_frame
            interaction_desktop_ui = analysis.runtime.interaction_desktop_ui
            interaction_map_ui = analysis.runtime.interaction_map_ui
            redraw_frame = analysis.runtime.redraw_frame
            assert interaction_frame is not None
            assert interaction_desktop_ui is not None
            assert interaction_map_ui is not None
            assert redraw_frame is not None
            self.assertEqual(interaction_frame.p95, 16.0)
            self.assertEqual(interaction_desktop_ui.p95, 1.1)
            self.assertEqual(interaction_map_ui.p95, 0.6)
            self.assertEqual(redraw_frame.maximum, 120.0)
            self.assertEqual(analysis.cpu.exclusive['garmin_ui::activity::map::paint'], 2)
            self.assertEqual(analysis.cpu.inclusive['core::iter::outer'], 2)
            self.assertEqual(analysis.cpu.crates['garmin_ui'], 2)
            self.assertEqual(analysis.cpu.total_samples, 2)
            self.assertEqual(analysis.cpu.sampled_cpu_seconds, 0.002)

            changed_manifest = deepcopy(analysis.manifest)
            changed_manifest['tools']['samply'] = 'different'
            changed = replace(analysis, directory=report / 'changed', manifest=changed_manifest)
            self.assertEqual(
                comparison_mismatches([analysis, changed]),
                [f'changed: Samply version differs from {analysis.directory.name}'],
            )

            (report / METRICS_NAME).write_text('tampered\n')
            with self.assertRaisesRegex(RuntimeError, 'does not match its manifest'):
                analyze_report(report)


if __name__ == '__main__':
    unittest.main()
