"""Disposable HTTPS OCI registry. Images are protocol fixtures, never bootable recipes."""
import gzip
import hashlib
import io
import json
import os
import pathlib
import shutil
import ssl
import subprocess
import tarfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def digest(data):
    return 'sha256:' + hashlib.sha256(data).hexdigest()


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        registry = self.server.registry
        registry.requests.append(self.path)
        prefix = '/v2/fixture/app/'
        body = None
        kind = 'application/octet-stream'
        if self.path.startswith(prefix + 'manifests/'):
            ref = self.path[len(prefix + 'manifests/'):]
            body = registry.manifests.get(registry.tags.get(ref, ref))
            kind = 'application/vnd.oci.image.manifest.v1+json'
        elif self.path.startswith(prefix + 'blobs/'):
            ref = self.path[len(prefix + 'blobs/'):]
            body = registry.corrupt.get(ref, registry.blobs.get(ref))
        if body is None:
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header('Content-Type', kind)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)


class Registry:
    def __init__(self, directory):
        self.root = pathlib.Path(directory)
        self.root.mkdir()
        self.ca = self.root / 'registry.crt'
        key = self.root / 'registry.key'
        openssl = shutil.which('openssl')
        env = os.environ.copy()
        if openssl is None and os.name == 'nt':
            candidate = pathlib.Path(r'C:\Program Files\Git\usr\bin\openssl.exe')
            if candidate.is_file():
                openssl = str(candidate)
                env['OPENSSL_CONF'] = r'C:\Program Files\Git\usr\ssl\openssl.cnf'
        if openssl is None:
            raise RuntimeError('OpenSSL is required for the disposable OCI TLS fixture')
        result = subprocess.run([
            openssl, 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1',
            '-keyout', str(key), '-out', str(self.ca), '-subj', '/CN=127.0.0.1',
            '-addext', 'subjectAltName=IP:127.0.0.1',
        ], env=env, capture_output=True, text=True, timeout=30)
        if result.returncode:
            raise RuntimeError('Cannot generate disposable registry certificate: ' + result.stderr)
        key.chmod(0o600)
        self.blobs = {}
        self.manifests = {}
        self.tags = {}
        self.corrupt = {}
        self.requests = []
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(self.ca, key)
        self.server.socket = context.wrap_socket(self.server.socket, server_side=True)
        self.server.registry = self
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.repository = '127.0.0.1:' + str(self.server.server_port) + '/fixture/app'

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)

    def image(self, version):
        layer_buffer = io.BytesIO()
        with tarfile.open(fileobj=layer_buffer, mode='w') as archive:
            for path, data, mode in [
                ('bin/busybox', b'protocol fixture, not a real binary', 0o755),
                ('app/version', version.encode(), 0o644),
                ('docker-entrypoint.sh', b'#!/bin/sh\nexec "$@"\n', 0o755),
                ('usr/local/bin/app', b'#!/bin/sh\ncat /app/version\n', 0o755),
            ]:
                info = tarfile.TarInfo(path)
                info.size, info.mode = len(data), mode
                archive.addfile(info, io.BytesIO(data))
            for path in ['bin/sh', 'sbin/init']:
                info = tarfile.TarInfo(path)
                info.type, info.linkname, info.mode = tarfile.SYMTYPE, '/bin/busybox', 0o777
                archive.addfile(info)
        layer = gzip.compress(layer_buffer.getvalue(), mtime=0)
        config = json.dumps({'architecture': 'amd64', 'os': 'linux', 'config': {
            'User': 'root', 'Env': ['IMAGE_SETTING=from-image'], 'WorkingDir': '/app',
            'Entrypoint': ['/docker-entrypoint.sh'], 'Cmd': ['/usr/local/bin/app'],
        }}, separators=(',', ':')).encode()
        manifest = json.dumps({'schemaVersion': 2,
            'mediaType': 'application/vnd.oci.image.manifest.v1+json',
            'config': {'mediaType': 'application/vnd.oci.image.config.v1+json', 'digest': digest(config), 'size': len(config)},
            'layers': [{'mediaType': 'application/vnd.oci.image.layer.v1.tar+gzip', 'digest': digest(layer), 'size': len(layer)}],
        }, separators=(',', ':')).encode()
        self.blobs[digest(config)] = config
        self.blobs[digest(layer)] = layer
        self.manifests[digest(manifest)] = manifest
        self.tags[version] = digest(manifest)
        return {'digest': digest(manifest), 'config': digest(config), 'layer': digest(layer)}


def read_template(data):
    """Read the converted archive without extracting or executing its contents."""
    result = {}
    with tarfile.open(fileobj=io.BytesIO(data), mode='r:xz') as archive:
        for item in archive:
            stream = archive.extractfile(item) if item.isfile() else None
            result[item.name] = {'mode': item.mode, 'link': item.linkname,
                                 'data': stream.read() if stream else None}
    return result
