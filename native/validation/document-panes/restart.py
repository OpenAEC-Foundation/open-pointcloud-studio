#!/usr/bin/env python3
"""Verify a native pane layout across two processes in one isolated profile."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import urllib.request
from PIL import Image
import numpy as np

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('binary',type=Path)
parser.add_argument('--station-source',type=Path,help='Optional real E57 with station 0 photographs')
parser.add_argument('--output',type=Path,required=True)
args=parser.parse_args()
output=args.output.resolve()
output.mkdir(parents=True,exist_ok=True)
binary=args.binary.resolve()
source=args.station_source.resolve() if args.station_source else output/'fixture.xyz'
if not args.station_source:
    source.write_text(''.join(f'{x/10} {y/10} {z/10} {x*25} {y*25} {z*25}\n'
                            for x in range(10) for y in range(10) for z in range(10)))
source_hash=hashlib.sha256(source.read_bytes()).hexdigest()
photo=output/'panorama.png'
image=Image.new('RGB',(512,256))
pixels=image.load()
for y in range(256):
    for x in range(512):
        pixels[x,y]=((230,70,120),(40,130,200), (80,190,90), (220,180,40))[(x//64+y//64)%4]
image.save(photo)

with tempfile.TemporaryDirectory(prefix='ops-layout-restart-') as directory:
    root=Path(directory)
    env={**os.environ,'XDG_CONFIG_HOME':str(root/'config'),'XDG_CACHE_HOME':str(root/'cache'),'OPENAEC_BEHEERD':'1'}
    def launch(name):
        with socket.socket() as sock:
            sock.bind(('127.0.0.1',0))
            port=sock.getsockname()[1]
        log=(output/f'{name}.log').open('w')
        process=subprocess.Popen([str(binary),'--api-port',str(port)],env=env,stdout=log,stderr=log)
        discovery=root/'config/open-pointcloud-studio-native/instances'/f'instance-{process.pid}.json'
        deadline=time.monotonic()+90
        while not discovery.exists():
            assert process.poll() is None, f'{name} exited; inspect its log'
            assert time.monotonic()<deadline
            time.sleep(.1)
        token=json.loads(discovery.read_text())['token']
        def api(command,**arguments):
            request=urllib.request.Request(f'http://127.0.0.1:{port}/exec',
                json.dumps({'command':command,**arguments}).encode(),
                {'Content-Type':'application/json','X-OPS-Token':token})
            with urllib.request.urlopen(request,timeout=90) as response:
                answer=json.load(response)
            assert answer.get('ok'), answer
            return answer
        return process,log,api
    def stop(process,log):
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
                raise
        log.close()
        assert process.returncode==0, process.returncode
    def wait_photo(api):
        deadline=time.monotonic()+90
        while True:
            panes=api('list_panes')
            if any(document.get('station_panorama_ready' if args.station_source else 'photo_ready')
                   for document in panes['documents']):
                return
            assert time.monotonic()<deadline, panes
            time.sleep(.1)
    first,log,api=launch('first')
    try:
        api('set_auto_index',enabled=False)
        api('open',path=str(source))
        deadline=time.monotonic()+90
        while True:
            state=api('status')['result']
            if state['clouds'] and not state.get('imports'):
                break
            assert time.monotonic()<deadline
            time.sleep(.1)
        if not args.station_source:
            api('add_panorama',layer=0,path=str(photo),position=[0,0,2],name='Persistent panorama')
        api('set_camera',yaw=.4,pitch=.5,zoom=.9,pan=[18,-6])
        if args.station_source:
            api('open_panorama',index=0,station=0)
        else:
            api('enter_photo',index=0)
        api('set_panorama',yaw=1.2,pitch=.15,field_of_view=1.1)
        if not args.station_source:
            api('photo_blend',value=.37)
        api('split_pane',side='right')
        api('show_tab',index=0)
        wait_photo(api)
        api('screenshot',path=str(output/'before.png'),base64=False,max_edge=1200,window=True)
        model=api('status')['result']['camera']
        before_panes=api('list_panes')
        model_look=next(document['look']['camera'] for document in before_panes['documents'] if document['key']=='model')
        api('show_tab',index=1)
        photo_state=api('status')['result']
        walk=photo_state['walk']
        blend=photo_state['photos']['blend']
        api('show_tab',index=0)
        workspace=api('list_panes')['workspace']
    finally:
        stop(first,log)
    layout_path=root/'config/open-pointcloud-studio-native/document-layout-v1.json'
    assert layout_path.exists(), 'Exit did not save the layout'
    saved=json.loads(layout_path.read_text())
    (output/'saved-layout.json').write_text(json.dumps(saved,indent=2)+'\n')
    second,log,api=launch('second')
    try:
        deadline=time.monotonic()+90
        while True:
            panes=api('list_panes')
            assert panes['persistence']['error'] is None, panes
            if panes['enabled'] and not panes['persistence']['restoring']:
                break
            assert time.monotonic()<deadline, panes
            time.sleep(.1)
        wait_photo(api)
        assert panes['workspace']==workspace, (panes['workspace'],workspace)
        api('screenshot',path=str(output/'restored-model.png'),base64=False,max_edge=1200,window=True)
        restored_camera=api('status')['result']['camera']
        restored_panes=api('list_panes')
        restored_look=next(document['look']['camera'] for document in restored_panes['documents'] if document['key']=='model')
        (output/'camera-comparison.json').write_text(json.dumps({'before':model,'after':restored_camera,
            'before_look':model_look,'after_look':restored_look},indent=2)+'\n')
        for key in ('yaw','pitch','zoom','orbit_point','view'):
            assert restored_camera[key]==model[key], (restored_camera,model)
        for axis in (0,1):
            before=model_look['pan'][axis]/model_look['reference_size'][axis]
            after=restored_look['pan'][axis]/restored_look['reference_size'][axis]
            assert abs(before-after)<1e-6, (axis,before,after)

        api('show_tab',index=1)
        photo_state=api('status')['result']
        reopened=photo_state['walk']
        if not args.station_source:
            assert photo_state['photos']['blend']==blend, (photo_state['photos'],blend)
        for key in ('eye','yaw','pitch','field_of_view'):
            assert reopened[key]==walk[key], (key,reopened,walk)
        api('show_tab',index=0)
        api('screenshot',path=str(output/'after.png'),base64=False,max_edge=1200,window=True)
        before_pixels=np.asarray(Image.open(output/'before.png').convert('RGB')).astype(np.int16)
        after_pixels=np.asarray(Image.open(output/'after.png').convert('RGB')).astype(np.int16)
        assert before_pixels.shape==after_pixels.shape
        height,width=before_pixels.shape[:2]
        counts={}
        for name,columns in [('model',(.2,.49)),('photo',(.51,.79))]:
            region=(slice(int(height*.45),int(height*.9)),slice(int(width*columns[0]),int(width*columns[1])))
            counts[name]=int(np.count_nonzero(np.any(np.abs(before_pixels[region]-after_pixels[region])>10,axis=2)))
        assert counts=={'model':0,'photo':0}, counts
        assert hashlib.sha256(source.read_bytes()).hexdigest()==source_hash
        result={'workspace_restored':True,'sources_reopened':True,'model_camera_restored':True,
                'photo_camera_restored':True,
                'station_source':bool(args.station_source),'render_changed_pixels':counts,'source_sha256':source_hash,'source_unchanged':True}
        if not args.station_source:
            result['photo_blend_restored']=True
        (output/'result.json').write_text(json.dumps(result,indent=2)+'\n')
        print(json.dumps(result,indent=2))
    finally:
        stop(second,log)
