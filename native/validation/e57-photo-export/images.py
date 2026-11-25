"""Read image payload hashes and poses using independent libE57 bindings."""
import hashlib
from pye57 import E57, libe57

def string(node):return libe57.StringNode(node).value()

def number(node):
    try:return libe57.FloatNode(node).value()
    except Exception:return libe57.IntegerNode(node).value()

def images(path):
    f=E57(str(path));root=f.image_file.root();result=[]
    if not root.isDefined('images2D'):return result
    vector=libe57.VectorNode(root.get('images2D'))
    for i in range(vector.childCount()):
        image=libe57.StructureNode(vector.get(i));record={}
        for key in ['name','associatedData3DGuid']:
            if image.isDefined(key):record[key]=string(image.get(key))
        if image.isDefined('pose'):
            pose=libe57.StructureNode(image.get('pose'));translation=libe57.StructureNode(pose.get('translation'))
            record['position']=[number(translation.get(k)) for k in ['x','y','z']]
            rotation=libe57.StructureNode(pose.get('rotation'))
            record['orientation']=[number(rotation.get(k)) for k in ['w','x','y','z']]
        for kind in ['pinholeRepresentation','sphericalRepresentation','cylindricalRepresentation','visualReferenceRepresentation']:
            if image.isDefined(kind):
                rep=libe57.StructureNode(image.get(kind))
                for encoding in ['jpegImage','pngImage','imageMask']:
                    if rep.isDefined(encoding):
                        blob=libe57.BlobNode(rep.get(encoding));record[kind+':'+encoding]=hashlib.sha256(blob.read_buffer().tobytes()).hexdigest()
        result.append(record)
    return result
