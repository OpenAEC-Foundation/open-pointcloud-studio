#!/usr/bin/env python3
"""Run the vendor export probe in a disposable, isolated native window."""
import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('binary', type=Path)
parser.add_argument('directory', type=Path)
parser.add_argument('--static', type=Path)
args = parser.parse_args()
binary = str(args.binary.resolve())
directory = args.directory.resolve()
with socket.socket() as sock:
    sock.bind(('127.0.0.1', 0))
    port = sock.getsockname()[1]
with tempfile.TemporaryDirectory(prefix='ops-e57-proof-') as work:
    env = {**os.environ, 'XDG_CONFIG_HOME': work + '/config',
           'XDG_CACHE_HOME': work + '/cache', 'PYTHONDONTWRITEBYTECODE': '1'}
    with (directory / 'validation-window.log').open('w') as log:
        process = subprocess.Popen([binary, '--api-port', str(port)], env=env,
                                   stdout=log, stderr=log)
        try:
            discovery = Path(work) / 'config/open-pointcloud-studio-native/instances' / f'instance-{process.pid}.json'
            deadline = time.monotonic() + 120
            while True:
                if process.poll() is not None:
                    raise RuntimeError('Test window exited; inspect validation-window.log')
                try:
                    instance = json.loads(discovery.read_text())
                    request = urllib.request.Request(f'http://127.0.0.1:{port}/exec',
                        json.dumps({'command': 'status'}).encode(),
                        {'Content-Type': 'application/json', 'X-OPS-Token': instance['token']})
                    with urllib.request.urlopen(request, timeout=15) as response:
                        if json.load(response).get('ok'):
                            break
                except (OSError, ValueError):
                    pass
                if time.monotonic() >= deadline:
                    raise RuntimeError('Test window did not become ready')
                time.sleep(.3)
            command = [sys.executable, str(Path(__file__).with_name('validate.py')),
                       str(directory), binary, str(discovery)]
            if args.static:
                command.extend(['--static', str(args.static.resolve())])
            result = subprocess.run(command, env=env)
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
sys.exit(result.returncode)
