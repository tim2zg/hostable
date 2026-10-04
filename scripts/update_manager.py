#!/usr/bin/env python3
"""Hostable's separate update worker. Standard library only; never invoked by a guest recipe."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import time
import urllib.request

REPOSITORY = 'https://github.com/tim2zg/hostable'
LIMIT = 128 * 1024 * 1024


def atomic_json(path, value):
    temporary = path.with_suffix('.tmp')
    with temporary.open('w', encoding='utf-8') as stream:
        json.dump(value, stream)
        stream.flush()
        os.fsync(stream.fileno())
    temporary.chmod(0o600)
    os.replace(temporary, path)


def write_status(root, request, phase, message):
    atomic_json(root / 'updates' / 'operation.json', {
        'version': request['version'], 'stage': request['stage'], 'phase': phase,
        'updated_at': int(time.time()), 'message': message,
    })


def validate_request(request, root):
    if not re.fullmatch(r'v\d+\.\d+\.\d+', request['version']):
        raise ValueError('Only stable version tags are supported')
    stage = Path(request['stage']).resolve()
    if stage.parent != root / 'updates' or not re.fullmatch(r'update_[A-Za-z0-9]+', stage.name):
        raise ValueError('Invalid update staging directory')
    for key, name in [('binary_url', 'hostable-linux-amd64'), ('checksum_url', 'hostable-linux-amd64.sha256')]:
        if request[key] != REPOSITORY + '/releases/download/' + request['version'] + '/' + name:
            raise ValueError('Untrusted release asset URL')
    if not isinstance(request['port'], int) or not 1 <= request['port'] <= 65535:
        raise ValueError('Invalid manager port')
    return stage


def download(url, target, limit):
    request = urllib.request.Request(url, headers={'User-Agent': 'Hostable-updater'})
    size = 0
    with urllib.request.urlopen(request, timeout=30) as response, target.open('wb') as stream:
        if not response.geturl().startswith('https://'):
            raise ValueError('Release asset redirected away from HTTPS')
        while True:
            chunk = response.read(64 * 1024)
            if not chunk:
                break
            size += len(chunk)
            if size > limit:
                raise ValueError('Release asset exceeds download limit')
            stream.write(chunk)
        stream.flush()
        os.fsync(stream.fileno())
    target.chmod(0o600)


def verify_candidate(stage, version):
    binary = stage / 'candidate'
    checksum = (stage / 'checksum').read_text(encoding='utf-8').strip().split()
    if len(checksum) != 2 or not re.fullmatch(r'[a-fA-F0-9]{64}', checksum[0]) or checksum[1] != 'hostable-linux-amd64':
        raise ValueError('Invalid release checksum file')
    with binary.open('rb') as stream:
        header = stream.read(20)
        stream.seek(0)
        digest = hashlib.sha256()
        for chunk in iter(lambda: stream.read(64 * 1024), b''):
            digest.update(chunk)
    if digest.hexdigest() != checksum[0].lower():
        raise ValueError('Release checksum mismatch')
    if len(header) < 20 or header[:6] != b'\x7fELF\x02\x01' or int.from_bytes(header[18:20], 'little') != 62:
        raise ValueError('Release must be a Linux x86-64 ELF binary')
    binary.chmod(0o700)
    result = subprocess.run([str(binary), '--version'], capture_output=True, text=True, timeout=10)
    if result.returncode or result.stdout.strip() != 'hostable ' + version:
        raise ValueError('Release version does not match the reviewed update')


def service(action):
    if Path('/run/systemd/system').is_dir():
        command = ['systemctl', action, 'hostable.service']
    else:
        command = ['rc-service', 'hostable', action]
    subprocess.run(command, check=True, capture_output=True, timeout=60)


def snapshot(root, stage):
    # The manager is stopped, so SQLite/WAL, credentials and certificates are consistent.
    with tarfile.open(stage / 'manager-backup.tar.gz', 'w:gz', dereference=False) as archive:
        for path in sorted(root.iterdir()):
            if path.name not in {'updates', 'cache', 'automation', 'backups'}:
                archive.add(path, arcname=path.name)
    (stage / 'manager-backup.tar.gz').chmod(0o600)
    with tarfile.open(stage / 'manager-backup.tar.gz', 'r:gz') as archive:
        checked_members(archive)


def checked_members(archive):
    members = archive.getmembers()
    for item in members:
        path = Path(item.name)
        if not path.parts or path.is_absolute() or '..' in path.parts or path.parts[0] in {'updates', 'cache', 'automation', 'backups'}:
            raise ValueError('Invalid manager backup member')
        if not (item.isfile() or item.isdir()):
            raise ValueError('Manager data links or devices require manual recovery')
    return members


def restore(root, stage):
    with tarfile.open(stage / 'manager-backup.tar.gz', 'r:gz') as archive:
        # Only this worker creates the backup. Reject paths and links which could escape it.
        members = checked_members(archive)
        for path in root.iterdir():
            if path.name in {'updates', 'cache', 'automation', 'backups'}:
                continue
            if path.is_symlink() or path.is_file():
                path.unlink()
            elif path.is_dir():
                shutil.rmtree(path)
        archive.extractall(root, members=members)


def healthy(root, port, version):
    token = (root / 'admin-token').read_text(encoding='utf-8').strip()
    for _ in range(30):
        try:
            with urllib.request.urlopen('http://127.0.0.1:' + str(port) + '/api/health', timeout=2) as response:
                data = json.load(response)
            if data.get('version') != version:
                raise ValueError('Unexpected running version')
            request = urllib.request.Request('http://127.0.0.1:' + str(port) + '/api/ready', headers={'Authorization': 'Bearer ' + token})
            with urllib.request.urlopen(request, timeout=2) as response:
                if json.load(response).get('status') == 'ready':
                    return
        except Exception:
            time.sleep(2)
    raise RuntimeError('Updated manager did not become ready')


def apply(root, stage, request, installed=Path('/usr/local/bin/hostable')):
    import fcntl
    with (root / 'updates' / 'worker.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        status = json.loads((root / 'updates/operation.json').read_text(encoding='utf-8'))
        if status['stage'] != str(stage) or status['phase'] != 'queued':
            raise ValueError('Only the recorded queued update can begin cutover')
        verify_candidate(stage, request['version'])
        previous = stage / 'previous'
        backup_ready = replaced = stopped = False
        write_status(root, request, 'applying', 'Stopping manager and saving its configuration and metadata')
        try:
            service('stop')
            stopped = True
            # Refuse links before any replacement; restoring must stay within manager data.
            for top in root.iterdir():
                if top.name in {'updates', 'cache', 'automation', 'backups'}:
                    continue
                paths = [top] + (list(top.rglob('*')) if top.is_dir() and not top.is_symlink() else [])
                if any(path.is_symlink() for path in paths):
                    raise ValueError('Manager data links require a manual upgrade')
            snapshot(root, stage)
            backup_ready = True
            shutil.copy2(installed, previous)
            previous.chmod(0o700)
            temporary = installed.with_suffix('.new')
            shutil.copy2(stage / 'candidate', temporary)
            temporary.chmod(0o755)
            os.replace(temporary, installed)
            replaced = True
            service('start')
            healthy(root, request['port'], request['version'])
            write_status(root, request, 'succeeded', 'Manager updated; previous binary and metadata backup retained')
        except Exception as error:
            if replaced:
                service('stop')
                temporary = installed.with_suffix('.rollback')
                shutil.copy2(previous, temporary)
                temporary.chmod(0o755)
                os.replace(temporary, installed)
                if backup_ready:
                    restore(root, stage)
            if stopped:
                service('start')
                healthy(root, request['port'], request['current_version'])
            write_status(root, request, 'failed', 'Update failed; previous installation restored: ' + type(error).__name__)
            raise


def recover(root, stage, request, installed=Path('/usr/local/bin/hostable')):
    import fcntl
    with (root / 'updates' / 'worker.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        status = json.loads((root / 'updates/operation.json').read_text(encoding='utf-8'))
        if status['stage'] != str(stage) or status['phase'] not in {'queued', 'applying', 'interrupted'}:
            raise ValueError('Only the unfinished update can be recovered')
        previous = stage / 'previous'
        if previous.is_file():
            with tarfile.open(stage / 'manager-backup.tar.gz', 'r:gz') as archive:
                checked_members(archive)
            service('stop')
            temporary = installed.with_suffix('.rollback')
            shutil.copy2(previous, temporary)
            temporary.chmod(0o755)
            os.replace(temporary, installed)
            restore(root, stage)
            service('start')
            healthy(root, request['port'], request['current_version'])
        write_status(root, request, 'failed', 'Interrupted update recovered; previous installation retained')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('action', choices=['stage', 'apply', 'recover'])
    parser.add_argument('request')
    args = parser.parse_args()
    os.umask(0o077)
    if os.name != 'posix' or os.geteuid() != 0:
        raise RuntimeError('Updates require the installed Linux root service')
    source = Path(args.request).resolve()
    root = source.parent.parent.parent
    request = json.loads(source.read_text(encoding='utf-8'))
    stage = validate_request(request, root)
    if source.parent != stage:
        raise ValueError('Request is outside its staging directory')
    if args.action == 'stage':
        if shutil.disk_usage(root).free < 2 * LIMIT:
            raise RuntimeError('Insufficient space to stage and retain a manager release')
        download(request['checksum_url'], stage / 'checksum', 4096)
        download(request['binary_url'], stage / 'candidate', LIMIT)
        verify_candidate(stage, request['version'])
    elif args.action == 'recover':
        recover(root, stage, request)
    else:
        # OpenRC workers detach immediately; leave time for the API response to finish.
        time.sleep(5)
        try:
            apply(root, stage, request)
        except Exception:
            # Keep failures before cutover visible as well. Never report a rollback as success.
            status = json.loads((root / 'updates' / 'operation.json').read_text(encoding='utf-8'))
            if status['phase'] != 'failed':
                write_status(root, request, 'interrupted', 'Update worker failed; inspect retained binary and metadata backup')
            raise


if __name__ == '__main__':
    main()
