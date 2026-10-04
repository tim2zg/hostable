"""Update worker acceptance with private files and simulated service transitions."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('hostable_update_worker', Path(__file__).resolve().parents[1] / 'scripts/update_manager.py')
worker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(worker)


class UpdateWorker(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='hostable-updater-')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve() / 'manager'
        self.stage = self.root / 'updates' / 'update_fixture'
        self.stage.mkdir(parents=True)
        self.request = {'version': 'v1.5.20', 'current_version': 'v1.5.19', 'stage': str(self.stage), 'port': 3000,
                        'binary_url': worker.REPOSITORY + '/releases/download/v1.5.20/hostable-linux-amd64',
                        'checksum_url': worker.REPOSITORY + '/releases/download/v1.5.20/hostable-linux-amd64.sha256'}
        self.installed = Path(self.temporary.name).resolve() / 'installed-binary'
        self.installed.write_bytes(b'previous manager')
        (self.stage / 'candidate').write_bytes(b'new manager')
        (self.root / 'metadata.db').write_bytes(b'previous metadata')
        (self.root / '.env').write_bytes(b'previous configuration')
        (self.root / 'admin-token').write_bytes(b'fixture token')
        worker.write_status(self.root, self.request, 'queued', 'fixture verified release')
        if os.name == 'nt':
            # File locking is provided by fcntl on the Linux worker. Windows tests
            # exercise the file/backup/restart algorithm without a Linux service.
            context = patch.dict(sys.modules, {'fcntl': SimpleNamespace(LOCK_EX=1, LOCK_NB=2, flock=lambda *args: None)})
            context.start()
            self.addCleanup(context.stop)

    def test_update_worker_rejects_foreign_urls_and_stage_escape(self):
        self.assertEqual(worker.validate_request(self.request, self.root), self.stage)
        for key, value in [('binary_url', 'https://example.invalid/binary'), ('version', 'v1.5.20;sh'), ('stage', str(self.root.parent / 'elsewhere'))]:
            with self.subTest(key=key), self.assertRaises(ValueError):
                worker.validate_request(dict(self.request, **{key: value}), self.root)

    def test_candidate_integrity_is_checked_before_any_execution(self):
        data = bytearray(20)
        data[:6] = b'\x7fELF\x02\x01'
        data[18:20] = (62).to_bytes(2, 'little')
        (self.stage / 'candidate').write_bytes(data)
        digest = hashlib.sha256(data).hexdigest()
        (self.stage / 'checksum').write_text(digest + '  hostable-linux-amd64\n', encoding='utf-8')
        with patch.object(worker.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, 'hostable v1.5.20\n', '')) as run:
            worker.verify_candidate(self.stage, 'v1.5.20')
            self.assertEqual(run.call_count, 1)
            (self.stage / 'candidate').write_bytes(b'corrupt release')
            with self.assertRaises(ValueError):
                worker.verify_candidate(self.stage, 'v1.5.20')
            self.assertEqual(run.call_count, 1)

    def test_wrong_reported_version_is_rejected(self):
        data = bytearray(20)
        data[:6] = b'\x7fELF\x02\x01'
        data[18:20] = (62).to_bytes(2, 'little')
        (self.stage / 'candidate').write_bytes(data)
        (self.stage / 'checksum').write_text(hashlib.sha256(data).hexdigest() + '  hostable-linux-amd64\n', encoding='utf-8')
        with patch.object(worker.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, 'hostable v1.5.19\n', '')):
            with self.assertRaises(ValueError):
                worker.verify_candidate(self.stage, 'v1.5.20')

    def test_success_retains_binary_and_consistent_metadata_backup(self):
        with patch.object(worker, 'verify_candidate'), patch.object(worker, 'service') as service, patch.object(worker, 'healthy'):
            worker.apply(self.root, self.stage, self.request, self.installed)
        self.assertEqual(self.installed.read_bytes(), b'new manager')
        self.assertEqual((self.stage / 'previous').read_bytes(), b'previous manager')
        self.assertTrue((self.stage / 'manager-backup.tar.gz').is_file())
        self.assertEqual([x.args[0] for x in service.call_args_list], ['stop', 'start'])
        self.assertEqual(json.loads((self.root / 'updates/operation.json').read_text())['phase'], 'succeeded')

    def test_cancelled_pending_worker_cannot_start_a_later_cutover(self):
        worker.recover(self.root, self.stage, self.request, self.installed)
        with patch.object(worker, 'verify_candidate') as verify, patch.object(worker, 'service') as service:
            with self.assertRaises(ValueError):
                worker.apply(self.root, self.stage, self.request, self.installed)
        verify.assert_not_called()
        service.assert_not_called()
        self.assertEqual(self.installed.read_bytes(), b'previous manager')
        self.assertEqual(json.loads((self.root / 'updates/operation.json').read_text())['phase'], 'failed')

    def test_failed_readiness_restores_binary_configuration_metadata_and_token(self):
        def migrate(action):
            if action == 'start' and self.installed.read_bytes() == b'new manager':
                (self.root / 'metadata.db').write_bytes(b'new schema')
                (self.root / 'admin-token').write_bytes(b'changed token')
                (self.root / 'new-file').write_bytes(b'added during migration')
        with patch.object(worker, 'verify_candidate'), patch.object(worker, 'service', side_effect=migrate) as service, patch.object(worker, 'healthy', side_effect=[RuntimeError('not ready'), None]):
            with self.assertRaises(RuntimeError):
                worker.apply(self.root, self.stage, self.request, self.installed)
        self.assertEqual(self.installed.read_bytes(), b'previous manager')
        self.assertEqual((self.root / 'metadata.db').read_bytes(), b'previous metadata')
        self.assertEqual((self.root / '.env').read_bytes(), b'previous configuration')
        self.assertEqual((self.root / 'admin-token').read_bytes(), b'fixture token')
        self.assertFalse((self.root / 'new-file').exists())
        self.assertEqual([x.args[0] for x in service.call_args_list], ['stop', 'start', 'stop', 'start'])
        self.assertEqual(json.loads((self.root / 'updates/operation.json').read_text())['phase'], 'failed')

    def test_failed_backup_keeps_previous_installation(self):
        with patch.object(worker, 'verify_candidate'), patch.object(worker, 'snapshot', side_effect=OSError('full disk')), patch.object(worker, 'service') as service, patch.object(worker, 'healthy'):
            with self.assertRaises(OSError):
                worker.apply(self.root, self.stage, self.request, self.installed)
        self.assertEqual(self.installed.read_bytes(), b'previous manager')
        self.assertEqual([x.args[0] for x in service.call_args_list], ['stop', 'start'])

    def test_interrupted_cutover_recovers_only_the_recorded_pending_update(self):
        worker.snapshot(self.root, self.stage)
        (self.stage / 'previous').write_bytes(self.installed.read_bytes())
        self.installed.write_bytes(b'new manager')
        (self.root / 'metadata.db').write_bytes(b'new schema')
        worker.write_status(self.root, self.request, 'interrupted', 'fixture host restart')
        with patch.object(worker, 'service') as service, patch.object(worker, 'healthy'):
            worker.recover(self.root, self.stage, self.request, self.installed)
            with self.assertRaises(ValueError):
                worker.recover(self.root, self.stage, self.request, self.installed)
        self.assertEqual(self.installed.read_bytes(), b'previous manager')
        self.assertEqual((self.root / 'metadata.db').read_bytes(), b'previous metadata')
        self.assertEqual([x.args[0] for x in service.call_args_list], ['stop', 'start'])


if __name__ == '__main__':
    unittest.main()
