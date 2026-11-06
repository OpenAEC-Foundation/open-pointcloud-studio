#!/usr/bin/env python3
"""Validate the actual native atlas preview and independently reopen OBJ UVs.

Requires Pillow, NumPy and trimesh. Starts and cleans up its own isolated window.
Run under a Wayland compositor or an X11 display with a Vulkan-capable renderer.
"""
import argparse
import hashlib
import json
import os
import re
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
parser.add_argument('--cache-root', type=Path, help='Private persistent cache, optionally preindexed')
parser.add_argument('--vertices', type=int, default=300)
parser.add_argument('--texel-size', type=float, default=.2)
parser.add_argument('--max-resolution', type=int, default=1024)
parser.add_argument('--job-timeout', type=float, default=180)
parser.add_argument('--cancel-probe', action='store_true')
parser.add_argument('--pointer-probe', type=Path, help='Optional isolated Wayland virtual pointer test helper')
parser.add_argument('--pointer-layout-width', type=int, help='Total logical width of the isolated horizontal output layout')
parser.add_argument('--desktop-capture', type=Path, help='Optional grim binary for screenshots of the isolated test compositor')
parser.add_argument('--cross-window-probe', action='store_true', help='Drag a native satellite tab back using the isolated labwc pointer helper')
parser.add_argument('--windows-probe', action='store_true', help='Check native satellite creation, capture selection and docking')
parser.add_argument('--panes-probe', action='store_true', help='Check separate native document panes and camera ownership')
parser.add_argument('--compare-probe', action='store_true', help='Check independently prepared model/photo cameras in the real renderer')
parser.add_argument('--expected-sha256', help='Verify the original real fixture after the probe')
args = parser.parse_args()
output = args.output.resolve()
output.mkdir(parents=True, exist_ok=True)
with socket.socket() as socket_:
    socket_.bind(('127.0.0.1', 0))
    port = socket_.getsockname()[1]
with tempfile.TemporaryDirectory(prefix='ops-photo-texture-') as temporary:
    root = Path(temporary)
    env = {**os.environ, 'XDG_CONFIG_HOME': str(root/'config'),
           'XDG_CACHE_HOME': str(args.cache_root.resolve() if args.cache_root else root/'cache')}
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
                        if error.code != 504 or command not in ('status','screenshot','job') or time.monotonic()>=deadline:
                            raise
                        time.sleep(.2)
                assert reply.get('ok'), reply
                return reply
            progress_samples=[]
            def wait_job(reply, accepted_states=('complete',)):
                started=time.monotonic()
                deadline = started+args.job_timeout
                next_sample=0.
                while time.monotonic()<deadline:
                    reply_ = api('job', id=reply['job_id'])
                    value = reply_.get('result') or reply_.get('job') or reply_
                    if time.monotonic()>=next_sample:
                        progress_samples.append({'seconds':round(time.monotonic()-started,3),**{key:value[key] for key in ('operation','mode','state','processed','total','stage') if key in value}})
                        if len(progress_samples)>240:
                            del progress_samples[0]
                        next_sample=time.monotonic()+2
                    if value.get('state') in accepted_states:
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
            if args.compare_probe or args.panes_probe:
                api('enter_photo', index=0)
                if args.panes_probe:
                    api('set_photo_compare', enabled=False)
                    panes=api('list_panes') if args.pointer_probe else api('split_pane', side='right')
                    assert bool(panes['enabled']) != bool(args.pointer_probe), panes
                    report['document_panes']=panes
                    if args.pointer_probe:
                        # This helper targets only this isolated compositor's
                        # WAYLAND_DISPLAY; the user's desktop is never selected.
                        subprocess.run(['wlrctl','toplevel','maximize','state:active'],check=True)
                        time.sleep(1)
                        api('status')
                        api('screenshot',path=str(output/'drag-before.png'),base64=False,max_edge=8192,window=True)
                        width,height=Image.open(output/'drag-before.png').size
                        outputs=subprocess.check_output(['wlrctl','output','list'],text=True)
                        mode=re.search(r'\((\d+)x(\d+) ',outputs)
                        assert mode, outputs
                        main_output_width,output_height=map(int,mode.groups())
                        output_width=args.pointer_layout_width or main_output_width
                        # labwc's server decorations are outside Iced's screenshot.
                        offset_y=output_height-height
                        assert main_output_width==width and output_width>=main_output_width and 0<=offset_y<=64, (outputs,width,height)
                        header=32+28+94+14
                        panel=240
                        center_y=int((height+header)/2)
                        def drag(start,end,client=True):
                            offset=offset_y if client else 0
                            subprocess.run([str(args.pointer_probe),'move',str(start[0]),str(start[1]+offset),str(output_width),str(output_height),
                                'down','wait','300','move',str(start[0]+8),str(start[1]+offset),str(output_width),str(output_height),
                                'move',str(end[0]),str(end[1]+offset),str(output_width),str(output_height),'wait','300','drop',str(end[0]),str(end[1]+offset),str(output_width),str(output_height),'wait','300'],check=True)
                            time.sleep(.3)
                        # Split directly from the original shared tab strip;
                        # no API split or enable action precedes this press.
                        drag((135,header),(width-panel-8,center_y))
                        initial=api('list_panes')
                        api('screenshot',path=str(output/'drag-initial-strip.png'),base64=False,max_edge=8192,window=True)
                        assert initial['enabled'], initial
                        root=initial['workspace']['windows'][0]['root']
                        assert root['kind']=='split', initial
                        assert root['first']['pane']['tabs']==['model'], initial
                        assert root['second']['pane']['active']!='model', initial
                        drag((int(width/2)+40,header),(panel+int((width-2*panel)/4),center_y))
                        merged=api('list_panes')
                        api('screenshot',path=str(output/'drag-merged.png'),base64=False,max_edge=8192,window=True)
                        assert merged['workspace']['windows'][0]['root']['kind']=='pane', merged
                        api('show_tab',index=0)
                        api('screenshot',path=str(output/'drag-model-selected.png'),base64=False,max_edge=8192,window=True)
                        drag((panel+135,header),(width-panel-8,center_y))
                        split=api('list_panes')
                        api('screenshot',path=str(output/'drag-split.png'),base64=False,max_edge=8192,window=True)
                        root=split['workspace']['windows'][0]['root']
                        assert root['kind']=='split', split
                        assert root['first']['pane']['tabs']==['model'], split
                        assert root['second']['pane']['active']!='model', split
                        report['native_tab_drag']={'initial_strip_split':True,'merged_into_pane':True,'inactive_photo_dragged_to_edge':True}

                else:
                    api('set_photo_compare', enabled=True)
                    api('set_photo_split', ratio=.5, vertical=False)
                api('set_panorama', yaw=.2, pitch=.1, field_of_view=1.2)
                # Let the newly decoded photograph settle before capturing.
                time.sleep(.5)
                first=api('status')['result']
                model_camera=first['photos']['model_camera']
                assert model_camera is not None
                api('screenshot',path=str(output/'compare-before.png'),base64=False,max_edge=1200,window=True)
                api('set_panorama',yaw=1.4,pitch=.1,field_of_view=1.2)
                turned=api('status')['result']
                assert turned['photos']['model_camera']==model_camera, 'Photo turn changed model camera'
                photo_camera=turned['walk']
                api('screenshot',path=str(output/'compare-photo-turn.png'),base64=False,max_edge=1200,window=True)
                # set_camera controls the model tab, not its comparison copy.
                # Change the actual model and return to the preserved photo.
                api('show_tab',index=0)
                api('set_camera',yaw=.4,pitch=.5,zoom=1,pan=[0,0])
                api('show_tab',index=1)
                time.sleep(.5)
                model_turned=api('status')['result']
                assert model_turned['walk']==photo_camera, 'Model turn changed photo camera'
                assert model_turned['photos']['model_camera']!=model_camera
                api('screenshot',path=str(output/'compare-model-turn.png'),base64=False,max_edge=1200,window=True)
                captures=[np.asarray(Image.open(output/name).convert('RGB')).astype(np.int16)
                          for name in ('compare-before.png','compare-photo-turn.png','compare-model-turn.png')]
                assert captures[0].shape==captures[1].shape==captures[2].shape
                height,width=captures[0].shape[:2]
                # Interior regions of this fixed two-pane fixture exclude
                # chrome, the divider, compass, photo controls and status.
                left=(slice(int(height*.45),int(height*.9)),slice(int(width*.2),int(width*.49)))
                right=(slice(int(height*.45),int(height*.9)),slice(int(width*.51),int(width*.79)))
                def changed_pixels(a,b,region):
                    return int(np.count_nonzero(np.any(np.abs(a[region]-b[region])>10,axis=2)))
                counts={'photo_turn_left':changed_pixels(captures[0],captures[1],left),
                        'photo_turn_right':changed_pixels(captures[0],captures[1],right),
                        'model_turn_left':changed_pixels(captures[1],captures[2],left),
                        'model_turn_right':changed_pixels(captures[1],captures[2],right)}
                assert counts['photo_turn_left']==0 and counts['model_turn_right']==0, counts
                assert counts['photo_turn_right']>100 and counts['model_turn_left']>100, counts
                report['independent_comparison_cameras']=counts
                if args.windows_probe:
                    torn=api('tear_off_tab')
                    windows=torn['workspace']['windows']
                    assert len(windows)==2, torn
                    satellite=next(window['id'] for window in windows if window['id']!=0)
                    time.sleep(.5)
                    api('screenshot',path=str(output/'satellite-photo.png'),base64=False,max_edge=1200,window=True)
                    satellite_size=Image.open(output/'satellite-photo.png').size
                    api('show_tab',index=0)
                    assert api('status')['result']['walk'] is None
                    api('screenshot',path=str(output/'main-model.png'),base64=False,max_edge=1200,window=True)
                    main_size=Image.open(output/'main-model.png').size
                    assert main_size!=satellite_size, (main_size,satellite_size)
                    api('show_tab',index=1)
                    assert api('status')['result']['walk']==photo_camera
                    if args.cross_window_probe:
                        assert args.pointer_probe and args.panes_probe
                        # The isolated labwc session centers this 900px-wide
                        # satellite. Move its server titlebar right, exposing
                        # the main model pane, then drop the photo tab there.
                        if args.desktop_capture:
                            subprocess.run([str(args.desktop_capture),str(output/'desktop-satellite.png')],check=True)
                        drag((main_output_width//2,12),(output_width-180,12),client=False)
                        if args.desktop_capture:
                            subprocess.run([str(args.desktop_capture),str(output/'desktop-moved.png')],check=True)
                        satellite_left=output_width-180-satellite_size[0]//2
                        drag((satellite_left+40,14),(400,400))
                        if args.desktop_capture:
                            subprocess.run([str(args.desktop_capture),str(output/'desktop-drop.png')],check=True)
                        docked=api('list_panes')
                        api('screenshot',path=str(output/'cross-window-docked.png'),base64=False,max_edge=8192,window=True)
                        assert len(docked['workspace']['windows'])==1, docked
                        assert docked['workspace']['windows'][0]['root']['kind']=='pane', docked
                        assert api('status')['result']['walk']==photo_camera
                        report['native_drag_back']={'implicit_grab_release_handled':True,'single_main_pane':True,'photo_camera_preserved':True,'virtual_desktop_width':output_width,'main_output_width':main_output_width,'across_outputs':output_width>main_output_width}
                    else:
                        docked=api('dock_pane_window',window=satellite)
                    assert len(docked['workspace']['windows'])==1, docked
                    report['native_satellite']={'logical_window':satellite,'satellite_capture_size':satellite_size,
                        'main_capture_size':main_size,'docked':True,'photo_camera_preserved':True}

                api('set_photo_compare',enabled=False)
                api('close_panorama')
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
                api('set_surface_settings',max_vertices=args.vertices,neighbors=12,edge_factor=4,mesh_size=0)
                reconstruction_started=time.monotonic()
                surface=wait_job(api('mesh',mode='surface',path=str(output/'real-surface.obj')))
                reconstruction_seconds=time.monotonic()-reconstruction_started
                texture_started=time.monotonic()
                real=wait_job(api('texture_mesh',path=str(output/'real-photo.obj'),source='photographs',
                                  texel_size=args.texel_size,max_resolution=args.max_resolution))
                texture_seconds=time.monotonic()-texture_started
                assert real['coverage']['photograph_texels']>0, real
                loaded=trimesh.load(str(output/'real-photo.obj'),force='scene',process=False)
                assert all(g.visual.kind=='texture' for g in loaded.geometry.values())
                report['real_e57']={'source':args.e57.name,'surface':surface,'texture':real,
                                   'independent_materials':len(loaded.geometry),
                                   'reconstruction_seconds':reconstruction_seconds,'texture_seconds':texture_seconds,
                                   'output_bytes':sum(item.stat().st_size for item in output.glob('real-photo*') if item.suffix in ('.obj','.mtl','.png')),
                                   'progress_samples':progress_samples}
                if args.cancel_probe:
                    before={item.name for item in output.iterdir()}
                    cancelled=api('texture_mesh',path=str(output/'cancelled.obj'),source='photographs',
                                  texel_size=.001,max_resolution=4096)
                    api('cancel_texture_mesh')
                    cancelled=wait_job(cancelled,('cancelled',))
                    assert {item.name for item in output.iterdir()}==before, 'Cancellation left export/staged files'
                    report['real_e57']['cancellation']={'state':cancelled['state'],'left_partial_files':False}
                status_path=Path(f'/proc/{process.pid}/status')
                if status_path.exists():
                    hwm=next(line for line in status_path.read_text().splitlines() if line.startswith('VmHWM:'))
                    report['native_peak_rss_kib']=int(hwm.split()[1])
                report['execution_profile']='development binary, core opt-level=2; Vulkan software renderer'
                (output/'result.json').write_text(json.dumps(report,indent=2)+'\n')
                api('screenshot',path=str(output/'real-photo.png'),base64=False,max_edge=1200,window=True)
            (output/'result.json').write_text(json.dumps(report,indent=2)+'\n')
            if args.e57 and args.expected_sha256:
                digest=hashlib.file_digest(args.e57.open('rb'),'sha256').hexdigest()
                assert digest==args.expected_sha256, 'Source fixture bytes changed'
                report['real_e57']['sha256']=digest
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
