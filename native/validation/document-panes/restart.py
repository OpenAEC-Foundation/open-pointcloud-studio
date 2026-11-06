#!/usr/bin/env python3
"""Verify a native pane layout across two processes in one isolated profile."""
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
from PIL import Image
import numpy as np

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('binary',type=Path)
parser.add_argument('--linked',action='store_true',help='Verify linked camera indicators, photo following and persistence')
parser.add_argument('--pointer-probe',type=Path,help='Optional isolated virtual pointer helper to click pane link controls')
parser.add_argument('--sheets',action='store_true',help='Also show two distinct sheets with placed drawings and paper annotations')
parser.add_argument('--drawings',action='store_true',help='Include independent plan/elevation panes and annotations')
parser.add_argument('--station-source',type=Path,help='Optional real E57 with station 0 photographs')
parser.add_argument('--output',type=Path,required=True)
args=parser.parse_args()
args.drawings=args.drawings or args.sheets
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
second_photo=output/'panorama-second.png'
if args.linked and not args.station_source:
    image.transpose(Image.Transpose.FLIP_TOP_BOTTOM).save(second_photo)

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
        if args.pointer_probe:
            # Discovery is published before the first native window maps.
            deadline=time.monotonic()+60
            while subprocess.run(['wlrctl','toplevel','maximize','state:active']).returncode:
                assert process.poll() is None, f'{name} exited before its window mapped'
                assert time.monotonic()<deadline, 'Native window did not map'
                time.sleep(.1)
            time.sleep(1)
            api('status')
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
    def wait_job(api, reply):
        deadline=time.monotonic()+90
        while True:
            value=api('job',id=reply['job_id'])
            value=value.get('result') or value.get('job') or value
            if value.get('state')=='complete':
                return value
            assert value.get('state') not in ('failed','cancelled'), value
            assert time.monotonic()<deadline, value
            time.sleep(.1)
    def interiors(path):
        pixels=np.asarray(Image.open(path).convert('RGB')).astype(np.int16)
        height,width=pixels.shape[:2]
        left,right=240,width-240
        top,bottom=198,height-30
        mid_x,mid_y=(left+right)//2,(top+bottom)//2
        rectangles={'model':(left,top,mid_x,mid_y),'photo':(mid_x,top,right,mid_y),
                    'plan':(left,mid_y,mid_x,bottom),'elevation':(mid_x,mid_y,right,bottom)}
        return {name:pixels[y1+20:y2-20,x1+20:x2-20] for name,(x1,y1,x2,y2) in rectangles.items()}
    def visible_regions(path):
        if args.drawings:
            return interiors(path)
        pixels=np.asarray(Image.open(path).convert('RGB')).astype(np.int16)
        height,width=pixels.shape[:2]
        return {name:pixels[int(height*.45):int(height*.9),int(width*a):int(width*b)]
                for name,a,b in [('model',.2,.49),('photo',.51,.79)]}
    def all_panes(workspace):
        def walk(node):
            return [node['pane']] if node['kind']=='pane' else walk(node['first'])+walk(node['second'])
        return [pane for window in workspace['windows'] for pane in walk(window['root'])]
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
            if args.linked:
                api('add_panorama',layer=0,path=str(second_photo),position=[.45,.45,.45],name='Linked panorama')
        api('set_camera',yaw=.4,pitch=.5,zoom=.9,pan=[18,-6])
        if args.drawings:
            assert not args.station_source, 'The 2 x 2 probe uses the measured XYZ fixture'
            wait_job(api,api('create_drawing',kind='plan',height=.45,thickness=.2,sample_percent=100,name='Restart plan'))
            api('split_pane',side='below')
            wait_job(api,api('create_drawing',kind='elevation',side='front',sample_percent=100,name='Restart elevation'))
            api('split_pane',side='right')
            api('show_tab',index=0)
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
        if args.sheets:
            sheet_guids={}
            for drawing,name,paper,orientation,at in [('Restart plan','Restart sheet A','a4','portrait',[105,140]),('Restart elevation','Restart sheet B','a3','landscape',[210,140])]:
                api('show_tab',name=drawing)
                sheet_guids[name]=api('create_sheet',name=name,paper=paper,orientation=orientation)['sheet']['guid']
                api('place_view',sheet=name,name=drawing,kind='drawing',at=at,scale=10)
            api('show_tab',index=0)
        if args.drawings:
            api('show_tab',name='Restart plan')
            api('set_drawing_layer',layer='OPS-FRAME',visible=False)
            if args.sheets:
                api('show_tab',index=next(tab['index'] for tab in api('list_tabs')['tabs'] if tab.get('guid')==sheet_guids['Restart sheet A']))
            api('show_tab',index=0)
        linked_report={}
        if args.linked:
            if not args.station_source:
                api('show_tab',index=next(tab['index'] for tab in api('list_tabs')['tabs'] if tab['kind']=='photo'))
                api('enter_photo',index=1,layer=0)
                api('show_tab',index=0)
                wait_photo(api)
            model_before=api('status')['result']['camera']
            api('screenshot',path=str(output/'link-off.png'),base64=False,max_edge=1200,window=True)
            state=api('list_panes')
            model_pane=next(pane for pane in all_panes(state['workspace']) if 'model' in pane['tabs'])
            photo_key=next(doc['key'] for doc in state['documents'] if doc['look'] and doc['look']['kind'] in ('photo','station'))
            photo_pane=next(pane for pane in all_panes(state['workspace']) if photo_key in pane['tabs'])
            if args.pointer_probe:
                api('screenshot',path=str(output/'link-controls.png'),base64=False,max_edge=8192,window=True)
                width,height=Image.open(output/'link-controls.png').size
                outputs=subprocess.check_output(['wlrctl','output','list'],text=True)
                mode=re.search(r'\((\d+)x(\d+) ',outputs)
                assert mode, outputs
                output_width,output_height=map(int,mode.groups())
                offset_y=output_height-height
                assert output_width==width and 0<=offset_y<=64
                for x in (width//2-36,width-240-36):
                    subprocess.run([str(args.pointer_probe),'move',str(x),str(168+offset_y),str(output_width),str(output_height),
                        'down','drop',str(x),str(168+offset_y),str(output_width),str(output_height),'wait','300'],check=True)
                linked=api('list_panes')
                assert all(pane['linked'] for pane in all_panes(linked['workspace']) if pane['id'] in (model_pane['id'],photo_pane['id'])), linked
                linked_report['native_link_controls_clicked']=True
                api('show_tab',index=0)
            else:
                for pane in (model_pane,photo_pane):
                    api('set_pane_linked',pane=pane['id'],linked=True)
            api('screenshot',path=str(output/'link-on.png'),base64=False,max_edge=1200,window=True)
            assert api('status')['result']['camera']==model_before
            assert next(doc for doc in api('list_panes')['documents'] if doc['key']=='model')['linked_photo_count']==1
            off,on=visible_regions(output/'link-off.png'),visible_regions(output/'link-on.png')
            indicator=int(np.count_nonzero(np.any(np.abs(off['model']-on['model'])>10,axis=2)))
            assert not np.any(np.abs(off['photo']-on['photo'])>10)
            if not args.station_source:
                assert indicator>0, indicator
            selected=api('open_panorama',index=0,station=0) if args.station_source else api('enter_photo',index=0,layer=0)
            assert selected['active_document']=='model', selected
            assert api('list_panes')['active_document']=='model'
            assert api('status')['result']['camera']==model_before
            wait_photo(api)
            api('screenshot',path=str(output/'linked-selection.png'),base64=False,max_edge=1200,window=True)
            if not args.station_source:
                followed=visible_regions(output/'linked-selection.png')
                changed=int(np.count_nonzero(np.any(np.abs(on['photo']-followed['photo'])>10,axis=2)))
                assert changed>100, changed
                linked_report['photo_follow_changed_pixels']=changed
                api('enter_photo',index=1,layer=0)
                wait_photo(api)
            linked_report.update(link_flags_restored=True,linked_scene_camera_unchanged=True,linked_selection_keeps_scene_active=True,camera_indicator_changed_pixels=indicator)
        if args.drawings:
            # Rendering settles each drawing's first-fit camera before comparing.
            api('screenshot',path=str(output/'without-notes.png'),base64=False,max_edge=1200,window=True)
            if args.sheets:
                for name in ('Restart sheet A','Restart sheet B'):
                    api('annotate_sheet',sheet=name,kind='text',at=[30,100],text=name,height=10)
            else:
                for name in ('Restart plan','Restart elevation'):
                    api('annotate_drawing',name=name,kind='text',at=[100,450],text=name,height=.5)
            api('screenshot',path=str(output/'with-notes.png'),base64=False,max_edge=1200,window=True)
            bare,marked=interiors(output/'without-notes.png'),interiors(output/'with-notes.png')
            annotation_pixels={name:int(np.count_nonzero(np.any(np.abs(bare[name]-marked[name])>10,axis=2))) for name in ('plan','elevation')}
            assert all(count>0 for count in annotation_pixels.values()), annotation_pixels
        api('screenshot',path=str(output/'before.png'),base64=False,max_edge=1200,window=True)
        model=api('status')['result']['camera']
        before_panes=api('list_panes')
        drawing_looks={doc['key']:doc['look'] for doc in before_panes['documents'] if doc['look'] and doc['look']['kind']=='drawing'} if args.drawings else {}
        model_look=next(document['look']['camera'] for document in before_panes['documents'] if document['key']=='model')
        api('show_tab',index=next(tab['index'] for tab in api('list_tabs')['tabs'] if tab['kind']=='photo'))
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
        if args.linked:
            assert next(doc for doc in panes['documents'] if doc['key']=='model')['linked_photo_count']==1
        api('screenshot',path=str(output/'restored-model.png'),base64=False,max_edge=1200,window=True)
        restored_camera=api('status')['result']['camera']
        restored_panes=api('list_panes')
        restored_look=next(document['look']['camera'] for document in restored_panes['documents'] if document['key']=='model')
        (output/'camera-comparison.json').write_text(json.dumps({'before':model,'after':restored_camera,
            'before_look':model_look,'after_look':restored_look},indent=2)+'\n')
        if args.drawings:
            restored_drawings={doc['key']:doc['look'] for doc in restored_panes['documents'] if doc['key'] in drawing_looks}
            assert len(drawing_looks)==(4 if args.sheets else 2), drawing_looks
            assert any(['OPS-FRAME',False] in look['layers'] for look in drawing_looks.values()), drawing_looks
            assert any(['OPS-FRAME',True] in look['layers'] for look in drawing_looks.values()), drawing_looks
            assert restored_drawings==drawing_looks, (restored_drawings,drawing_looks)
        for key in ('yaw','pitch','zoom','orbit_point','view'):
            assert restored_camera[key]==model[key], (restored_camera,model)
        for axis in (0,1):
            before=model_look['pan'][axis]/model_look['reference_size'][axis]
            after=restored_look['pan'][axis]/restored_look['reference_size'][axis]
            assert abs(before-after)<1e-6, (axis,before,after)

        api('show_tab',index=next(tab['index'] for tab in api('list_tabs')['tabs'] if tab['kind']=='photo'))
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
        if args.drawings:
            before_regions,after_regions=interiors(output/'before.png'),interiors(output/'after.png')
            counts={name:int(np.count_nonzero(np.any(np.abs(before_regions[name]-after_regions[name])>10,axis=2))) for name in before_regions}
            assert all(count==0 for count in counts.values()), counts
        else:
            assert counts=={'model':0,'photo':0}, counts
        assert hashlib.sha256(source.read_bytes()).hexdigest()==source_hash
        result={'workspace_restored':True,'sources_reopened':True,'model_camera_restored':True,
                'photo_camera_restored':True,
                'station_source':bool(args.station_source),'render_changed_pixels':counts,'source_sha256':source_hash,'source_unchanged':True}
        result.update(linked_report)
        if args.sheets:
            result.update(distinct_sheets_and_paper_cameras_restored=True,paper_annotations_restored=True)
        if args.drawings:
            result.update(drawing_cameras_and_layers_restored=True,annotations_restored=True,annotation_changed_pixels=annotation_pixels,four_panes_restored=True)
        if not args.station_source:
            result['photo_blend_restored']=True
        (output/'result.json').write_text(json.dumps(result,indent=2)+'\n')
        print(json.dumps(result,indent=2))
    finally:
        stop(second,log)
