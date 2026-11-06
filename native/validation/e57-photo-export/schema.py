"""Compare E57 prototype field types/limits through independent libE57."""
from pye57 import libe57


def point_schema(file, scan, omit_colour=False):
    header = file.get_header(scan)
    prototype = libe57.StructureNode(header.points.prototype())
    result = {}
    for name in header.point_fields:
        if omit_colour and (name.startswith('color') or name == 'isColorInvalid'):
            continue
        node = prototype.get(name)
        kind = node.type()
        if kind == libe57.NodeType.E57_FLOAT:
            value = libe57.FloatNode(node)
            result[name] = (str(kind), str(value.precision()), value.minimum(), value.maximum())
        elif kind == libe57.NodeType.E57_INTEGER:
            value = libe57.IntegerNode(node)
            result[name] = (str(kind), value.minimum(), value.maximum())
        elif kind == libe57.NodeType.E57_SCALED_INTEGER:
            value = libe57.ScaledIntegerNode(node)
            result[name] = (str(kind), value.minimum(), value.maximum(), value.scale(), value.offset())
        else:
            raise AssertionError(f'Unsupported prototype field type: {name}: {kind}')
    return result


def non_colour_schema(file, scan):
    return point_schema(file, scan, omit_colour=True)
