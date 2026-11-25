#!/usr/bin/env python3
"""Independently select the BLK2GO section in bounded-memory libE57 chunks."""
import argparse,json
from pathlib import Path
import numpy as np
from pye57 import E57
from images import images
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument("directory",type=Path)
args=parser.parse_args()
b=args.directory.resolve()
f=E57(str(b/"BLK2GO_Demo02.e57"));h=f.get_header(0);fields=["cartesianX","cartesianY","cartesianZ","intensity","colorRed","colorGreen","colorBlue"]
data,buffers=f.make_buffers(fields,500000);reader=h.points.reader(buffers)
lo=np.array([-5.1,-30,-1.5]);hi=np.array([-5.06,-20,-.01]);selected={k:[] for k in fields};total=0
while True:
 n=reader.read()
 if not n:break
 p=np.column_stack([data["cartesian"+k][:n] for k in "XYZ"]);mask=np.all((p>=lo)&(p<=hi),axis=1)
 if mask.any():
  for k in fields:selected[k].append(data[k][:n][mask].copy())
 total+=n
 if total%50000000==0:print("independent source points read",total,flush=True)
reader.close();selected={k:np.concatenate(v) for k,v in selected.items()};np.savez(b/"independent-section.npz",**selected)
records=images(b/"BLK2GO_Demo02.e57");(b/"source-images.json").write_text(json.dumps(records,indent=2));print("source points",total,"selected",len(selected["cartesianX"]),"images",len(records),flush=True)
