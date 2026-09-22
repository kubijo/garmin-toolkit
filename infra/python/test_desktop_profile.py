import gzip
import io
import json
import shlex
import subprocess
import tempfile
import unittest
from contextlib import redirect_stderr
from pathlib import Path
from unittest.mock import patch

from desktop_profile import (
    COMBINED_NAME,
    CURRENT_METRICS_SCHEMA_VERSION,
    MANIFEST_NAME,
    METRICS_NAME,
    PROFILE_NAME,
    ProfileCanceled,
    ProfileError,
    ReportRun,
    ReportSlot,
    capture,
    file_digest,
    finalize,
    main,
    report_summary,
    require_perf_access,
    run_checked,
)


class DesktopProfileTests(unittest.TestCase):
    def report(self) -> tuple[tempfile.TemporaryDirectory[str], Path]:
        temporary = tempfile.TemporaryDirectory()
        report = Path(temporary.name)
        profile = {
            'meta': {'startTime': 1_000.0},
            'threads': [
                {
                    'name': 'main',
                    'pid': '42',
                    'samples': {'length': 2},
                }
            ],
            'counters': [],
        }
        with gzip.open(report / PROFILE_NAME, 'wt') as destination:
            json.dump(profile, destination)
        rows = [
            {
                'kind': 'header',
                'schema_version': 1,
                'started_unix_milliseconds': 1_005.0,
            },
            {
                'kind': 'desktop_frame',
                'elapsed_milliseconds': 8.0,
                'frame_milliseconds': None,
                'ui_milliseconds': 4.0,
            },
            {
                'kind': 'desktop_frame',
                'elapsed_milliseconds': 9.0,
                'frame_milliseconds': 10.0,
                'ui_milliseconds': 3.5,
            },
            {
                'kind': 'map_frame',
                'elapsed_milliseconds': 10.0,
                'frame_milliseconds': None,
                'interaction_frame_milliseconds': None,
                'camera_active': False,
                'ui_milliseconds': 2.0,
                'scene_milliseconds': 0.5,
                'route_query_microseconds': 12.0,
                'label_milliseconds': 0.2,
                'label_backlog': 1,
                'stale_work': 2,
                'visible_tiles': 6,
                'ready_tiles': 5,
                'pending_tiles': 1,
                'queued_upload_bytes': 256,
                'uploaded_bytes': 128,
            },
            {
                'kind': 'map_frame',
                'elapsed_milliseconds': 20.0,
                'frame_milliseconds': 16.0,
                'interaction_frame_milliseconds': 16.0,
                'camera_active': True,
                'ui_milliseconds': 3.0,
                'scene_milliseconds': 0.75,
                'route_query_microseconds': 14.0,
                'label_milliseconds': 0.3,
                'label_backlog': 0,
                'stale_work': 3,
                'visible_tiles': 7,
                'ready_tiles': 7,
                'pending_tiles': 0,
                'queued_upload_bytes': 0,
                'uploaded_bytes': 512,
            },
            {
                'kind': 'map_render',
                'elapsed_milliseconds': 21.0,
                'phase': 'prepare',
                'milliseconds': 0.15,
            },
            {
                'kind': 'map_render',
                'elapsed_milliseconds': 22.0,
                'phase': 'draw',
                'milliseconds': 0.4,
            },
        ]
        (report / METRICS_NAME).write_text(''.join(f'{json.dumps(row)}\n' for row in rows))
        return temporary, report

    def test_enrichment_keeps_raw_artifacts_byte_identical(self) -> None:
        temporary, report = self.report()
        self.addCleanup(temporary.cleanup)
        raw_hashes = {name: file_digest(report / name) for name in (PROFILE_NAME, METRICS_NAME)}

        count = finalize(report)

        self.assertEqual(count, 18)
        self.assertEqual(raw_hashes, {name: file_digest(report / name) for name in raw_hashes})
        with gzip.open(report / COMBINED_NAME, 'rt') as source:
            combined = json.load(source)
        self.assertEqual(len(combined['counters']), 18)
        frame_counter = next(counter for counter in combined['counters'] if counter['name'] == 'Map frame interval')
        self.assertEqual(frame_counter['samples']['time'], [25.0])
        backlog = next(counter for counter in combined['counters'] if counter['name'] == 'Label backlog')
        self.assertEqual(backlog['samples']['count'], [1.0, -1.0])

    def test_report_summary_puts_copyable_commands_on_their_own_lines(self) -> None:
        temporary, report = self.report()
        self.addCleanup(temporary.cleanup)
        finalize(report)

        lines = report_summary(report).splitlines()
        analysis_command = shlex.join(('just', 'desktop::profile-analyze', report.name))
        combined_command = shlex.join(('samply', 'load', str((report / COMBINED_NAME).resolve())))
        raw_command = shlex.join(('samply', 'load', str((report / PROFILE_NAME).resolve())))

        self.assertIn(analysis_command, lines)
        self.assertIn(combined_command, lines)
        self.assertIn(raw_command, lines)
        self.assertEqual(
            lines[lines.index(combined_command) - 1], 'Open enriched profile (CPU samples + runtime counters)'
        )
        self.assertEqual(lines[lines.index(raw_command) - 1], 'Open raw Samply capture')

    def test_loads_current_metrics_schema(self) -> None:
        temporary, report = self.report()
        self.addCleanup(temporary.cleanup)
        metrics = report / METRICS_NAME
        rows = [json.loads(line) for line in metrics.read_text().splitlines()]
        rows[0]['schema_version'] = CURRENT_METRICS_SCHEMA_VERSION
        metrics.write_text('\n'.join(json.dumps(row) for row in rows) + '\n')

        self.assertEqual(finalize(report), 18)

    def test_current_metrics_schema_requires_interaction_fields(self) -> None:
        temporary, report = self.report()
        self.addCleanup(temporary.cleanup)
        metrics = report / METRICS_NAME
        rows = [json.loads(line) for line in metrics.read_text().splitlines()]
        rows[0]['schema_version'] = CURRENT_METRICS_SCHEMA_VERSION
        del rows[3]['camera_active']
        metrics.write_text('\n'.join(json.dumps(row) for row in rows) + '\n')

        with self.assertRaisesRegex(ProfileError, 'boolean camera_active'):
            finalize(report)

    def test_refuses_to_replace_an_existing_combined_profile(self) -> None:
        temporary, report = self.report()
        self.addCleanup(temporary.cleanup)
        (report / COMBINED_NAME).write_bytes(b'evidence')

        with self.assertRaises(ProfileError):
            finalize(report)

        self.assertEqual((report / COMBINED_NAME).read_bytes(), b'evidence')

    def test_rejects_metrics_without_map_samples(self) -> None:
        temporary, report = self.report()
        self.addCleanup(temporary.cleanup)
        (report / METRICS_NAME).write_text(
            json.dumps(
                {
                    'kind': 'header',
                    'schema_version': 1,
                    'started_unix_milliseconds': 1_005.0,
                }
            )
        )

        with self.assertRaises(ProfileError):
            finalize(report)

    def test_perf_guard_rejects_restricted_linux_sampling(self) -> None:
        with tempfile.TemporaryDirectory() as temporary, patch('desktop_profile.sys.platform', 'linux'):
            setting = Path(temporary) / 'perf_event_paranoid'
            setting.write_text('4\n')

            with self.assertRaises(ProfileError):
                require_perf_access(setting)

            setting.write_text('-1\n')
            self.assertEqual(require_perf_access(setting), '-1')

    def test_perf_guard_does_not_require_linux_settings_on_macos(self) -> None:
        with patch('desktop_profile.sys.platform', 'darwin'), patch('desktop_profile.Path.exists') as exists:
            self.assertIsNone(require_perf_access())
            exists.assert_not_called()

    def test_interrupted_build_creates_no_report(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            report = Path(temporary) / 'baseline'
            with (
                patch('desktop_profile.require_tool'),
                patch('desktop_profile.require_perf_access', return_value='-1'),
                patch('desktop_profile.report_directory', return_value=report),
                patch('desktop_profile.build_profiling_binary', side_effect=ProfileCanceled('profiling build')),
                self.assertRaises(ProfileCanceled),
            ):
                capture('demo', 'baseline', ())

            self.assertFalse(report.exists())

    def test_legacy_manifest_only_report_is_preserved_before_reuse(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            report = root / 'baseline'
            report.mkdir()
            manifest = report / MANIFEST_NAME
            manifest.write_text(json.dumps({'schema_version': 1, 'status': 'building'}))
            digest = file_digest(manifest)

            slot = ReportSlot.inspect(report)
            self.assertEqual(slot.recoverable_manifest_sha256, digest)
            slot.reserve()

            self.assertTrue(report.is_dir())
            self.assertEqual(list(report.iterdir()), [])
            abandoned = list(root.glob('.baseline.abandoned-*'))
            self.assertEqual(len(abandoned), 1)
            self.assertEqual(file_digest(abandoned[0] / MANIFEST_NAME), digest)

    def test_failed_manifest_only_report_is_preserved_before_reuse(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            report = root / 'baseline'
            report.mkdir()
            manifest = report / MANIFEST_NAME
            manifest.write_text(json.dumps({'schema_version': 2, 'status': 'failed'}))
            digest = file_digest(manifest)

            slot = ReportSlot.inspect(report)
            self.assertEqual(slot.recoverable_manifest_sha256, digest)
            slot.reserve()

            self.assertTrue(report.is_dir())
            self.assertEqual(list(report.iterdir()), [])
            abandoned = list(root.glob('.baseline.abandoned-*'))
            self.assertEqual(len(abandoned), 1)
            self.assertEqual(file_digest(abandoned[0] / MANIFEST_NAME), digest)

    def test_report_with_raw_evidence_is_never_reclaimed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            report = Path(temporary) / 'baseline'
            report.mkdir()
            (report / MANIFEST_NAME).write_text(json.dumps({'schema_version': 1, 'status': 'building'}))
            (report / PROFILE_NAME).write_bytes(b'evidence')

            with self.assertRaises(ProfileError):
                ReportSlot.inspect(report)

            self.assertEqual((report / PROFILE_NAME).read_bytes(), b'evidence')

    def test_report_state_machine_records_capture_cancellation(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            report = Path(temporary) / 'baseline'
            run = ReportRun.start(
                ReportSlot.inspect(report),
                {'schema_version': 2, 'status': 'capturing', 'phase': 'capturing'},
            )

            run.cancel('profile capture')

            manifest = json.loads((report / MANIFEST_NAME).read_text())
            self.assertEqual(manifest['status'], 'canceled')
            self.assertEqual(manifest['canceled_during'], 'profile capture')
            with self.assertRaises(ProfileError):
                run.transition('complete')

    def test_report_transition_is_not_published_in_memory_until_written(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            report = Path(temporary) / 'baseline'
            run = ReportRun.start(
                ReportSlot.inspect(report),
                {'schema_version': 2, 'status': 'capturing', 'phase': 'capturing'},
            )

            with (
                patch('desktop_profile.write_manifest', side_effect=OSError('disk full')),
                self.assertRaises(OSError),
            ):
                run.transition('finalizing')

            self.assertEqual(run.manifest['status'], 'capturing')
            manifest = json.loads((report / MANIFEST_NAME).read_text())
            self.assertEqual(manifest['status'], 'capturing')

    def test_sigint_exit_is_concise_and_uses_status_130(self) -> None:
        stderr = io.StringIO()
        with (
            patch('desktop_profile.build_only', side_effect=ProfileCanceled('profiling build')),
            patch('sys.argv', ['desktop_profile.py', 'build', 'demo']),
            redirect_stderr(stderr),
            self.assertRaises(SystemExit) as exit_status,
        ):
            main()

        self.assertEqual(exit_status.exception.code, 130)
        self.assertIn('Partial Cargo artifacts were retained', stderr.getvalue())
        self.assertNotIn('Traceback', stderr.getvalue())

    def test_sigint_return_code_is_normalized_as_cancellation(self) -> None:
        with (
            patch(
                'desktop_profile.subprocess.run',
                side_effect=subprocess.CalledProcessError(130, ['cargo']),
            ),
            self.assertRaises(ProfileCanceled),
        ):
            run_checked(['cargo'], phase='profiling build')


if __name__ == '__main__':
    unittest.main()
