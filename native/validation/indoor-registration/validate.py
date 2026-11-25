import sys,json,time,urllib.request,hashlib,platform
from pathlib import Path
import numpy as np
from scipy.spatial import cKDTree
from pye57 import E57
import argparse
parser=argparse.ArgumentParser(description="Oxford Spires Christ Church indoor registration validation")
parser.add_argument("data_directory", type=Path)
parser.add_argument("--config-directory", type=Path, required=True)
parser.add_argument("--port", type=int, default=42167)
args=parser.parse_args()
b=args.data_directory
instance=next((args.config_directory / "open-pointcloud-studio-native/instances").glob("instance-*.json"))
token=json.loads(instance.read_text())["token"]
def api(command,**args):
 req=urllib.request.Request(f"http://127.0.0.1:{args.port}/exec",json.dumps({"command":command,**args}).encode(),{"Content-Type":"application/json","X-OPS-Token":token})
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
fixed=np.load(b/"fixed.npy",mmap_mode="r");mobile=np.load(b/"mobile.npy")
# A deterministic 1-in-10 TLS subset keeps the viewport test small. The
# independent metric still uses every TLS point in the spatial overlap.
reference=np.asarray(fixed[::10],dtype="<f4")
with (b/"tls-validation.ply").open("wb") as f:
 f.write(f"ply\nformat binary_little_endian 1.0\nelement vertex {len(reference)}\nproperty float x\nproperty float y\nproperty float z\nend_header\n".encode());f.write(reference.tobytes())
seed=json.loads((b/"preparation-report.json").read_text())["seed"]
tree=cKDTree(fixed)
def metrics(tr):
 r=np.asarray(tr["rotation"]);t=np.asarray(tr["translation"]);d=tree.query(mobile@r.T+t,workers=2)[0]
 return {"mean_mm":float(d.mean()*1000),"median_mm":float(np.median(d)*1000),"p95_mm":float(np.quantile(d,.95)*1000),"within_50mm_percent":float((d<.05).mean()*100)}
report={"hardware":{"cpu":platform.processor(),"machine":platform.machine()},"before":metrics(seed),"reference_points":len(reference),"moving_points":len(mobile)}
print("before",report,flush=True)
api("set_auto_index",enabled=False)
api("open",path=str(b/"tls-validation.ply"));idle();api("open",path=str(b/"slam-overlap.ply"));idle()
api("registration_setup",fixed=0,moving=1);api("registration_set_matrix",transform=seed)
t=time.monotonic();report["job"]=job(api("registration_refine"));report["icp_seconds"]=time.monotonic()-t
report["registration"]=status()["registration"]
tr=report["registration"]["transform"];report["after"]=metrics(tr)
(b/"indoor-metrics.json").write_text(json.dumps(report,indent=2));print("after",report["after"],flush=True)
out=b/"indoor-registered.e57";job(api("registration_export",path=str(out),merge=True));idle()
f=E57(str(out));counts=[f.get_header(i).point_count for i in range(f.scan_count)];assert counts==[len(reference),len(mobile)],counts
scan=f.read_scan(1,ignore_missing_fields=True);actual=np.column_stack([scan["cartesian"+k] for k in "XYZ"])
expected=mobile.astype("f4").astype("f8")@np.array(tr["rotation"]).T+np.array(tr["translation"])
error=np.linalg.norm(actual-expected,axis=1);assert error.max()<1e-6,error.max()
report["export_counts"]=counts;report["export_max_coordinate_error_m"]=float(error.max());report["ok"]=True
for name in ["tls-1cm.pcd","slam-clouds.zip"]:
 h=hashlib.sha256()
 with (b/name).open("rb") as stream:
  for chunk in iter(lambda:stream.read(8*1024*1024),b""):h.update(chunk)
 report.setdefault("sha256",{})[name]=h.hexdigest()
(b/"validation-report.json").write_text(json.dumps(report,indent=2));print(json.dumps(report),flush=True)
