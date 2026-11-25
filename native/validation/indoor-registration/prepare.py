import sys, json, zipfile, time
from pathlib import Path
import numpy as np
from scipy.spatial.transform import Rotation
import argparse
parser=argparse.ArgumentParser(description="Oxford Spires Christ Church indoor registration validation")
parser.add_argument("data_directory", type=Path)
args=parser.parse_args()
b=args.data_directory
start=time.monotonic()
gt=np.loadtxt(b/"gt-tum.txt"); slam=np.loadtxt(b/"vilens-slam-tum.txt")
ids=np.searchsorted(gt[:,0],slam[:,0]);ids=np.clip(ids,1,len(gt)-1);ids-=(abs(gt[ids-1,0]-slam[:,0])<abs(gt[ids,0]-slam[:,0]))
mask=abs(gt[ids,0]-slam[:,0])<.06
p=slam[mask,1:4];q=gt[ids[mask],1:4];pc=p.mean(0);qc=q.mean(0)
u,sv,v=np.linalg.svd((p-pc).T@(q-qc));r=v.T@np.diag([1,1,np.linalg.det(v.T@u.T)])@u.T;t=qc-pc@r.T
z=zipfile.ZipFile(b/"slam-clouds.zip");names=sorted(n for n in z.namelist() if n.endswith(".pcd"))[40:100:2]
clouds=[]
for name in names:
 with z.open(name) as f:
  h={}
  while True:
   line=f.readline().decode().strip();parts=line.split()
   if not parts or parts[0].startswith("#"):continue
   h[parts[0]]=parts[1:]
   if parts[0]=="DATA":break
  assert h["DATA"]==["binary"]
  a=np.frombuffer(f.read(),dtype=np.dtype([(k,"<f4") for k in h["FIELDS"]]))
  p=np.column_stack([a[k] for k in "xyz"]);vp=np.array(h["VIEWPOINT"],float)
  rot=Rotation.from_quat([vp[4],vp[5],vp[6],vp[3]]).as_matrix()
  valid=np.isfinite(p).all(1)&(np.linalg.norm(p,axis=1)<8)&(np.linalg.norm(p,axis=1)>.7)
  clouds.append(p[valid]@rot.T+vp[:3])
mobile=np.concatenate(clouds);_,idx=np.unique(np.floor(mobile/.03).astype("i8"),axis=0,return_index=True);mobile=mobile[np.sort(idx)]
world=mobile@r.T+t;lo=world.min(0)-.3;hi=world.max(0)+.3
print("mobile",len(mobile),"ROI",lo.tolist(),hi.tolist(),flush=True)
with open(b/"tls-1cm.pcd","rb") as f:
 while not f.readline().startswith(b"DATA"):pass
 offset=f.tell()
a=np.memmap(b/"tls-1cm.pcd",mode="r",offset=offset,dtype=np.dtype([("intensity","<f4"),("rgb","<f4"),("x","<f4"),("y","<f4"),("z","<f4"),("padding","V4")]))
fixed=[]
for i in range(0,len(a),500000):
 p=np.column_stack([a[k][i:i+500000] for k in "xyz"]);valid=np.isfinite(p).all(1)&np.all((p>=lo)&(p<=hi),axis=1)
 if valid.any():fixed.append(p[valid])
fixed=np.concatenate(fixed)
def ply(path,p):
 p=np.asarray(p,dtype="<f4")
 with path.open("wb") as f:
  f.write(f"ply\nformat binary_little_endian 1.0\nelement vertex {len(p)}\nproperty float x\nproperty float y\nproperty float z\nend_header\n".encode());f.write(p.tobytes())
np.save(b/"mobile.npy",mobile);np.save(b/"fixed.npy",fixed);ply(b/"slam-overlap.ply",mobile);ply(b/"tls-overlap.ply",fixed)
report={"sequence":"2024-03-18-christ-church-03","tls_scanner":"Leica RTC360","slam_scanner":"Hesai QT64 / VILENS","trajectory_matches":int(mask.sum()),"seed":{"rotation":r.tolist(),"translation":t.tolist(),"scale":1},"mobile_points":len(mobile),"tls_full_points":len(a),"tls_subset_points":len(fixed),"roi_min":lo.tolist(),"roi_max":hi.tolist(),"mobile_frames":names,"elapsed_seconds":time.monotonic()-start}
(b/"preparation-report.json").write_text(json.dumps(report,indent=2));print(json.dumps(report),flush=True)
