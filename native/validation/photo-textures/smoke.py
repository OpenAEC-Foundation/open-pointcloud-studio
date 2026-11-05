#!/usr/bin/env python3
"""Validate the actual native atlas preview and independently reopen OBJ UVs.

Requires Pillow, NumPy and trimesh. Starts and cleans up its own isolated window.
Run under a Wayland compositor or an X11 display with a Vulkan-capable renderer.
"""
import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import urllib.request
import urllib.error

from PIL import Image, ImageChops
import numpy as np
import trimesh

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('binary', type=Path)
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--e57', type=Path, help='Optional real calibrated E57 fixture')
args = parser.parse_args()
output = args.output.resolve()
output.mkdir(parents=True, exist_ok=True)
with socket.socket() as socket_:
    socket_.bind(('127.0.0.1', 0))
    port = socket_.getsockname()[1]
with tempfile.TemporaryDirectory(prefix='ops-photo-texture-') as temporary:
    root = Path(temporary)
    env = {**os.environ, 'XDG_CONFIG_HOME': str(root/'config'),
           'XDG_CACHE_HOME': str(root/'cache')}
    with (output/'window.log').open('w') as log:
        process = subprocess.Popen([str(args.binary.resolve()), '--api-port', str(port)],
                                   env=env, stdout=log, stderr=log)
        try:
            discovery = root/'config/open-pointcloud-studio-native/instances'/f'instance-{process.pid}.json'
            deadline = time.monotonic()+120
            while not discovery.exists():
                assert process.poll() is None, 'Window exited; inspect window.log'
                assert time.monotonic()<deadline, 'Discovery timeout'
                time.sleep(.2)
            instance = json.loads(discovery.read_text())
            def api(command, **arguments):
                request = urllib.request.Request(f'http://127.0.0.1:{port}/exec',
                    json.dumps({'command': command, **arguments}).encode(),
                    {'Content-Type':'application/json', 'X-OPS-Token':instance['token']})
                deadline = time.monotonic()+90
                while True:
                    try:
                        with urllib.request.urlopen(request, timeout=90) as response:
                            reply = json.load(response)
                        break
                    except urllib.error.HTTPError as error:
                        # CPU Vulkan may exceed the command timeout compiling its
                        # first pipelines. Never replay a state-changing command.
                        if error.code != 504 or command not in ('status','screenshot') or time.monotonic()>=deadline:
                            raise
                        time.sleep(.2)
                assert reply.get('ok'), reply
                return reply
            def wait_job(reply):
                deadline = time.monotonic()+180
                while time.monotonic()<deadline:
                    reply_ = api('job', id=reply['job_id'])
                    value = reply_.get('result') or reply_.get('job') or reply_
                    if value.get('state')=='complete':
                        return value
                    assert value.get('state') not in ('failed','cancelled'), value
                    time.sleep(.2)
                raise RuntimeError('Job timeout')
            api('status')
            api('set_budget',points=1000)
            mesh = output/'measured.obj'
            mesh.write_text('v -1 -1 0 20 30 40\nv 1 -1 0 20 30 40\nv -1 1 0 20 30 40\nf 1 2 3\n')
            api('set_auto_index', enabled=False)
            api('open', path=str(mesh))
            deadline = time.monotonic()+60
            while True:
                state=api('status')['result']
                if state.get('clouds') and not state.get('imports'):
                    break
                assert time.monotonic()<deadline
                time.sleep(.2)
            panorama = output/'measured-panorama.png'
            image = Image.new('RGB',(256,128))
            image.putdata([(220 if x//16%2 else 40, 180 if y//16%2 else 30, 70)
                           for y in range(128) for x in range(256)])
            image.save(panorama)
            original = panorama.read_bytes()
            api('add_panorama', layer=0, path=str(panorama), position=[0,0,2], name='Checkerboard')
            path = output/'photographic.obj'
            complete = wait_job(api('texture_mesh', path=str(path), source='photographs',
                                    texel_size=.02, max_resolution=256))
            assert complete['coverage']['photograph_texels']>0, complete
            assert panorama.read_bytes()==original
            api('camera', preset='top')
            api('screenshot', path=str(output/'texture.png'), base64=False, max_edge=1200, window=False)
            api('set_texture_preview', layer=0, visible=False)
            api('screenshot', path=str(output/'geometry.png'), base64=False, max_edge=1200, window=False)
            difference=ImageChops.difference(Image.open(output/'texture.png').convert('RGB'),
                                            Image.open(output/'geometry.png').convert('RGB'))
            changed=int(np.count_nonzero(np.any(np.asarray(difference)>10,axis=2)))
            assert changed>100, f'Texture toggle did not change the viewport: {changed}'
            reopened=trimesh.load(str(path),force='scene',process=False)
            geometry=next(iter(reopened.geometry.values()))
            assert len(geometry.faces)==1
            assert geometry.visual.kind=='texture'
            assert geometry.visual.uv.shape==(3,2)
            assert geometry.visual.material.image.size==(256,256)
            assert np.isfinite(geometry.visual.uv).all()
            report = {'job':complete,
                'changed_viewport_pixels':changed,'independent_reader':'trimesh '+trimesh.__version__,
                'uvs':geometry.visual.uv.tolist(),'source_photo_unchanged':True}
            if args.e57:
                api('remove', index=0)
                api('open', path=str(args.e57.resolve()))
                deadline=time.monotonic()+180
                while True:
                    state=api('status')['result']
                    if state.get('clouds') and not state.get('imports'):
                        break
                    assert time.monotonic()<deadline
                    time.sleep(.2)
                api('set_surface_settings',max_vertices=300,neighbors=12,edge_factor=4,mesh_size=0)
                surface=wait_job(api('mesh',mode='surface',path=str(output/'real-surface.obj')))
                real=wait_job(api('texture_mesh',path=str(output/'real-photo.obj'),source='photographs',
                                  texel_size=.2,max_resolution=1024))
                assert real['coverage']['photograph_texels']>0, real
                loaded=trimesh.load(str(output/'real-photo.obj'),force='scene',process=False)
                assert all(g.visual.kind=='texture' for g in loaded.geometry.values())
                report['real_e57']={'source':args.e57.name,'surface':surface,'texture':real,
                                   'independent_materials':len(loaded.geometry)}
                (output/'result.json').write_text(json.dumps(report,indent=2)+'\n')
                api('screenshot',path=str(output/'real-photo.png'),base64=False,max_edge=1200,window=True)
            (output/'result.json').write_text(json.dumps(report,indent=2)+'\n')
            print('Photo atlas preview and independent OBJ/UV reload passed',flush=True)
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
