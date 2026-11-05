#!/usr/bin/env python3
import argparse,json,time,hashlib,urllib.request,subprocess,shutil
from pathlib import Path
import numpy as np
from pye57 import E57,libe57
from images import images
parser=argparse.ArgumentParser(description="Validate BLK2GO photo and raw-field export using a fresh test window")
parser.add_argument("directory", type=Path)
parser.add_argument("binary", type=Path)
parser.add_argument("discovery", type=Path, help="Discovery JSON of an isolated, empty test window")
parser.add_argument("--static", type=Path, help="Optional ManitouNoInvalidPoints.e57 static fixture")
args=parser.parse_args()
b=args.directory.resolve()
instance=json.loads(args.discovery.read_text())
token=instance["token"]
def api(command,**args):
 req=urllib.request.Request(f"http://127.0.0.1:{instance['port']}/exec",json.dumps({"command":command,**args}).encode(),{"Content-Type":"application/json","X-OPS-Token":token})
 with urllib.request.urlopen(req,timeout=30) as f:r=json.load(f)
 assert r.get("ok"),r
 return r
def status():return api("status")["result"]
def idle(timeout=600):
 until=time.monotonic()+timeout
 while time.monotonic()<until:
  s=status()
  if not s.get("imports") and not s.get("detail_pending") and not s.get("registration_pending"):return s
  time.sleep(.3)
 raise RuntimeError("window did not become idle")
def job(reply):
 until=time.monotonic()+600
 while time.monotonic()<until:
  r=api("job",id=reply["job_id"]);v=r.get("result") or r.get("job") or r
  if v.get("state")=="complete":return v
  if v.get("state") in ["failed","cancelled"]:raise RuntimeError(r)
  time.sleep(.3)
 raise RuntimeError("job timeout")

source_images=json.loads((b/"source-images.json").read_text());section=b/"section.e57";expected=np.load(b/"independent-section.npz")
f=E57(str(section));raw=f.read_scan_raw(0);assert len(raw["cartesianX"])==len(expected["cartesianX"])==471
for key in expected.files:
 np.testing.assert_allclose(raw[key],expected[key],rtol=0,atol=1e-10 if key.startswith("cartesian") else 0,err_msg=key)
def photo_hashes(records):return sorted(sorted((k,v) for k,v in row.items() if ":" in k) for row in records)
def associations(path):
 f=E57(str(path));scans=libe57.VectorNode(f.image_file.root().get("data3D"));guids={libe57.StringNode(libe57.StructureNode(scans.get(i)).get("guid")).value() for i in range(scans.childCount())}
 assert all(row.get("associatedData3DGuid") is None or row["associatedData3DGuid"] in guids for row in images(path))
records=images(section);assert len(records)==6;assert photo_hashes(records)==photo_hashes(source_images);associations(section)
for original,filtered in zip(source_images,records):
 for key in ["position","orientation"]:
  if key in original:np.testing.assert_array_equal(filtered[key],original[key])
(b/"section.e57.ops-photos.json").unlink(missing_ok=True)
report={"source_points":199186802,"section_points":471,"source_images":6,"section_image_bytes_unchanged":True,"section_attributes_exact":True}
print("section passes",report,flush=True)
assert status()["clouds"] == [], "Use an isolated empty test window"
api("set_auto_index",enabled=False);api("open",path=str(section));idle()
from PIL import Image
panorama=b/"added-panorama.png"
image=Image.new("RGB",(32,16))
image.putdata([(x*7,y*15,(x+y)*5) for y in range(16) for x in range(32)])
image.save(panorama)
api("add_panorama",layer=0,path=str(panorama),position=[0,0,1.6],name="Added test panorama")
listed=api("list_photos",layer=0);print("listed photos",listed,flush=True)
api("edit_photo",layer=0,index=0,name="Added test panorama edited",position=[0,0,1.6],heading=15)
job(api("colour_from_photos",layer=0,max_distance=100,blend=False));idle()
report["colour_job"]=status()["colour_from_photos"];print("colours",report["colour_job"],flush=True)
out=b/"edited-coloured.e57";job(api("export",path=str(out)));idle()
out_hashes={tuple(h) for h in photo_hashes(images(out))}
assert {tuple(h) for h in photo_hashes(source_images)} <= out_hashes
assert len(images(out))==7;associations(out)
assert any(row.get("name")=="Added test panorama edited" for row in images(out))
a=E57(str(out)).read_scan_raw(0);assert len(a["cartesianX"])==471
for key in expected.files:
 if not key.startswith("color"):np.testing.assert_allclose(a[key],expected[key],rtol=0,atol=1e-10 if key.startswith("cartesian") else 0)
report.update(added_panorama_edited_and_retained=True,edited_points=471,non_colour_attributes_exact=True)
matrix={"rotation":[[0,-1,0],[1,0,0],[0,0,1]],"translation":[10,20,30],"scale":1};(b/"matrix.json").write_text(json.dumps(matrix))
registered=b/"registered.e57";run=subprocess.run([str(args.binary.resolve()),"--apply-registration",str(out),str(b/"matrix.json"),str(registered)],capture_output=True,text=True);assert run.returncode==0,run.stderr
assert photo_hashes(images(registered))==photo_hashes(images(out));associations(registered)
registered_raw=E57(str(registered)).read_scan_raw(0)
for key in a:
 if not key.startswith("cartesian"):np.testing.assert_array_equal(registered_raw[key],a[key])
from scipy.spatial.transform import Rotation
for before,after in zip(images(out),images(registered)):
 np.testing.assert_allclose(np.asarray(before["position"])@np.asarray(matrix["rotation"]).T+matrix["translation"],after["position"],atol=1e-10)
 before_q=before["orientation"];after_q=after["orientation"]
 before_rotation=Rotation.from_quat([*before_q[1:],before_q[0]]).as_matrix()
 after_rotation=Rotation.from_quat([*after_q[1:],after_q[0]]).as_matrix()
 np.testing.assert_allclose(np.array(matrix["rotation"])@before_rotation,after_rotation,atol=1e-10)
report.update(registration_preserves_images_and_transforms_poses=True)
if args.static:
 static=b/"static-fixture.e57";shutil.copyfile(args.static,static)
 api("remove",index=0);api("open",path=str(static));idle()
 listed=api("list_photos",layer=0);assert len(listed["photos"])==5
 job(api("colour_from_photos",layer=0,max_distance=100,blend=True));idle()
 coloured=b/"static-coloured.e57";job(api("export",path=str(coloured)));idle()
 assert photo_hashes(images(coloured))==photo_hashes(images(static));associations(coloured)
 before=E57(str(static));after=E57(str(coloured));assert before.scan_count==after.scan_count==5
 for scan in range(before.scan_count):
  np.testing.assert_array_equal(before.get_header(scan).rotation_matrix,after.get_header(scan).rotation_matrix)
  np.testing.assert_array_equal(before.get_header(scan).translation,after.get_header(scan).translation)
 count=0
 for scan in range(before.scan_count):
  original=before.read_scan_raw(scan);edited=after.read_scan_raw(scan)
  for key in original:
   if not key.startswith("color"):np.testing.assert_array_equal(edited[key],original[key])
  count+=len(original["cartesianX"])
 report["static"]={"scans":5,"images":5,"points":count,"image_bytes_unchanged":True,"non_colour_attributes_exact":True}
report["ok"]=True
h=hashlib.sha256()
with (b/"BLK2GO_Demo02.e57").open("rb") as stream:
 for chunk in iter(lambda:stream.read(8*1024*1024),b""):h.update(chunk)
report["source_sha256"]=h.hexdigest();(b/"validation-report.json").write_text(json.dumps(report,indent=2));print(json.dumps(report),flush=True)
