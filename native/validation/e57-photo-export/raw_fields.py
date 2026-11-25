"""Read every supported raw point field through independent libE57 buffers."""
import numpy as np
from pye57 import libe57

def read_raw(file, scan):
 # pye57 omits this standard field from its convenience mapping. Read it
 # through libE57 directly instead of ignoring an unsupported attribute.
 header=file.get_header(scan);data={};buffers=libe57.VectorSourceDestBuffer()
 for field in header.point_fields:
  if field=="isColorInvalid":
   array=np.empty(header.point_count,np.int8)
   buffer=libe57.SourceDestBuffer(file.image_file,field,array,header.point_count,True,True)
  else:array,buffer=file.make_buffer(field,header.point_count)
  data[field]=array;buffers.append(buffer)
 reader=header.points.reader(buffers)
 assert reader.read()==header.point_count
 reader.close()
 if "isColorInvalid" in data:assert np.isin(data["isColorInvalid"],[0,1]).all()
 return data
