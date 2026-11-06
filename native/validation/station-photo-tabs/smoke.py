#!/usr/bin/env python3
"""Exercise a real E57 station in an isolated native GUI. Requires a display."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('binary', type=Path)
parser.add_argument('source', type=Path)
parser.add_argument('--output', required=True, type=Path)
args = parser.parse_args()
output = args.output.resolve()
output.mkdir(parents=True, exist_ok=True)
source = args.source.resolve()
def digest():
    with source.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()
before_hash = digest()
with socket.socket() as sock:
    sock.bind(('127.0.0.1', 0))
    port = sock.getsockname()[1]
with tempfile.TemporaryDirectory(prefix='ops-station-tabs-') as temporary:
    root = Path(temporary)
    env = {**os.environ, 'XDG_CONFIG_HOME': str(root/'config'), 'XDG_CACHE_HOME': str(root/'cache')}
    with (output/'window.log').open('w') as log:
        process = subprocess.Popen([str(args.binary.resolve()), '--api-port', str(port)],
                                   env=env, stdout=log, stderr=log)
        try:
            discovery = root/'config/open-pointcloud-studio-native/instances'/f'instance-{process.pid}.json'
            deadline = time.monotonic()+120
            while not discovery.exists():
                assert process.poll() is None, 'Native GUI exited; inspect window.log'
                assert time.monotonic()<deadline, 'API startup timed out'
                time.sleep(.2)
            token = json.loads(discovery.read_text())['token']
            def api(command, **arguments):
                request = urllib.request.Request(f'http://127.0.0.1:{port}/exec',
                    json.dumps({'command': command, **arguments}).encode(),
                    {'Content-Type': 'application/json', 'X-OPS-Token': token})
                deadline = time.monotonic()+90
                while True:
                    try:
                        with urllib.request.urlopen(request, timeout=90) as response:
                            reply = json.load(response)
                        break
                    except urllib.error.HTTPError as error:
                        # A timed-out mutation can still execute. Only repeat reads.
                        if error.code != 504 or command not in ('status', 'list_tabs', 'screenshot') or time.monotonic()>=deadline:
                            raise
                        time.sleep(.2)
                assert reply.get('ok'), reply
                return reply
            api('status')
            api('set_auto_index', enabled=False)
            api('open', path=str(source))
            deadline = time.monotonic()+120
            while True:
                state = api('status')['result']
                if state['clouds'] and not state.get('imports'):
                    break
                assert time.monotonic()<deadline, 'E57 import timed out'
                time.sleep(.2)
            api('camera', preset='top')
            model = api('status')['result']['camera']
            api('open_panorama', index=0, station=0)
            tabs = api('list_tabs')
            station = next(tab for tab in tabs['tabs'] if tab.get('photo_type')=='station_panorama')
            assert station['active'] and station['station_index']==0, tabs
            api('set_panorama', yaw=.8, pitch=.1, field_of_view=1.2)
            deadline = time.monotonic()+90
            while True:
                look = api('status')['result']['walk']
                if look.get('station', {}).get('full_resolution'):
                    break
                assert time.monotonic()<deadline, 'Station panorama did not decode'
                time.sleep(.2)
            api('screenshot', path=str(output/'station.png'), base64=False, max_edge=1200, window=True)
            api('show_tab', index=0)
            assert api('status')['result']['camera']==model, 'Model camera changed'
            api('show_tab', index=station['index'])
            restored = api('status')['result']['walk']
            for key in ('eye', 'yaw', 'pitch', 'field_of_view'):
                assert restored[key]==look[key], (key, restored, look)
            api('close_tab', index=station['index'])
            state = api('status')['result']
            assert state['camera']==model and len(state['clouds'])==1
            assert len(api('list_tabs')['tabs'])==1
            assert digest()==before_hash, 'Source E57 changed'
            result = {'source_sha256': before_hash, 'station_tab': {key:value for key,value in station.items() if key!='path'},
                      'model_camera_restored': True, 'photo_camera_restored': True,
                      'closing_preserves_scan': True, 'source_unchanged': True}
            (output/'result.json').write_text(json.dumps(result, indent=2)+'\n')
            print(json.dumps(result, indent=2))
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=15)
