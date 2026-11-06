#!/usr/bin/env python3
"""Compare all-valid E57 fixtures before/after native export in three external apps.

Requires numpy, scipy and pye57; PDAL with readers.e57; an E57-enabled
CloudCompare. Use a new output directory. Vendor files are read, never changed.
The three external integrations share the C++ libE57 family: they are not three
independent decoder implementations. Point records are checked exactly through
libE57; other readers' normalised/quantised outputs must match before and after.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import shutil

import numpy as np
from pye57 import E57, libe57
from scipy.spatial.transform import Rotation
from images import images
from raw_fields import read_raw
from schema import point_schema


def digest(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def scalar(node):
    kind = node.type()
    if kind == libe57.NodeType.E57_SCALED_INTEGER:
        return libe57.ScaledIntegerNode(node).scaledValue()
    if kind == libe57.NodeType.E57_INTEGER:
        return libe57.IntegerNode(node).value()
    if kind == libe57.NodeType.E57_FLOAT:
        return libe57.FloatNode(node).value()
    raise AssertionError(f'Unhandled scalar metadata type: {kind}')


def limits(header):
    result = {}
    for group in ['intensityLimits', 'colorLimits', 'indexBounds']:
        if not header.node.isDefined(group):
            continue
        node = libe57.StructureNode(header.node.get(group))
        result[group] = {node.get(i).elementName(): scalar(node.get(i))
                         for i in range(node.childCount())}
    return result


def calibrations(file):
    root = file.image_file.root()
    if not root.isDefined('images2D'):
        return []
    vector = libe57.VectorNode(root.get('images2D'))
    result = []
    for i in range(vector.childCount()):
        image = libe57.StructureNode(vector.get(i))
        record = {}
        for name in ['pinholeRepresentation', 'sphericalRepresentation',
                     'cylindricalRepresentation', 'visualReferenceRepresentation']:
            if not image.isDefined(name):
                continue
            rep = libe57.StructureNode(image.get(name))
            record[name] = {}
            for j in range(rep.childCount()):
                value = rep.get(j)
                if value.type() != libe57.NodeType.E57_BLOB:
                    record[name][value.elementName()] = scalar(value)
        result.append(record)
    return result


def compare_libe57(source, output):
    before, after = E57(str(source)), E57(str(output))
    assert before.scan_count == after.scan_count
    guids, scans = {}, []
    for i in range(before.scan_count):
        a, b = before.get_header(i), after.get_header(i)
        assert a.point_count == b.point_count
        guids[libe57.StringNode(a.node.get('guid')).value()] = libe57.StringNode(b.node.get('guid')).value()
        left, right = read_raw(before, i), read_raw(after, i)
        assert left.keys() == right.keys()
        for name in left:
            np.testing.assert_array_equal(left[name], right[name], err_msg=name)
        assert point_schema(before, i) == point_schema(after, i)
        assert limits(a) == limits(b), 'Physical limits or sensor grid frame changed'
        assert a.node.isDefined('pose') == b.node.isDefined('pose')
        if a.node.isDefined('pose'):
            np.testing.assert_array_equal(a.translation, b.translation)
            np.testing.assert_allclose(a.rotation_matrix, b.rotation_matrix, rtol=0, atol=1e-12)
        scans.append({'points': a.point_count, 'fields': list(left),
                      'point_values_and_schema_exact': True, 'physical_limits': limits(a),
                      'station_pose_unchanged': True})
    original, exported = images(source), images(output)
    assert len(original) == len(exported)
    for a, b in zip(original, exported):
        if a.get('associatedData3DGuid') is not None:
            assert a['associatedData3DGuid'] in guids
        expected_guid = guids.get(a.get('associatedData3DGuid'))
        assert b.get('associatedData3DGuid') == expected_guid
        for name in a.keys() | b.keys():
            if name == 'associatedData3DGuid':
                continue
            if name == 'orientation':
                convert = lambda q: Rotation.from_quat([*q[1:], q[0]]).as_matrix()
                np.testing.assert_allclose(convert(a[name]), convert(b[name]), rtol=0, atol=1e-12)
            else:
                assert a.get(name) == b.get(name), name
    assert calibrations(before) == calibrations(after)
    return {'scans': scans, 'images': len(original), 'image_bytes_and_calibration_exact': True,
            'image_pose_and_scan_associations_unchanged': True}


def run(command, log, env):
    with log.open('w') as stream:
        result = subprocess.run(command, env=env, stdout=stream, stderr=subprocess.STDOUT, timeout=300)
    assert result.returncode == 0, f'{command[0]} failed; inspect {log}'


def text_rows(path):
    count = 0
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            count += block.count(b'\n')
    return count - 1  # Each supported text writer emits one header line.


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path)
    parser.add_argument('--pdal', type=Path, required=True)
    parser.add_argument('--cloudcompare', type=Path, required=True)
    parser.add_argument('--cloudcompare-version', default='not reported')
    parser.add_argument('--source', type=Path, nargs='+', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--max-points', type=int, default=2_000_000,
                        help='Bound files passed to the external in-memory readers')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    env = {**os.environ, 'QT_QPA_PLATFORM': 'offscreen',
           'XDG_CONFIG_HOME': str(output/'config'), 'XDG_DATA_HOME': str(output/'data'),
           'XDG_CACHE_HOME': str(output/'cache')}
    plugins = args.cloudcompare.resolve().parent/'plugins'
    if (plugins/'platforms').is_dir():
        env['QT_QPA_PLATFORM_PLUGIN_PATH'] = str(plugins/'platforms')
        env['QT_PLUGIN_PATH'] = str(plugins)
    version = subprocess.check_output([str(args.pdal.resolve()), '--version'], text=True).strip()
    report = {'native_binary_sha256': digest(args.binary),
              'pdal': version, 'cloudcompare': args.cloudcompare_version,
              'cloudcompare_binary_sha256': digest(args.cloudcompare),
              'decoder_lineage': 'Three external applications; shared C++ libE57 family, independent of the native Rust decoder',
              'reader_limits': 'PDAL quantises intensity/RGB to uint16; CloudCompare uses float32 coordinates and uint8 RGB. libE57 raw records/schema and physical metadata are checked separately.',
              'system': platform.platform(), 'external_reader_point_limit': args.max_points, 'cases': []}
    e57_plugin = plugins/'CC/libQE57_IO_PLUGIN.so'
    if e57_plugin.exists():
        report['cloudcompare_e57_plugin_sha256'] = digest(e57_plugin)
    for index, original in enumerate(args.source):
        original = original.resolve()
        source = E57(str(original))
        count = sum(source.get_header(i).point_count for i in range(source.scan_count))
        assert 0 < count <= args.max_points, 'Use a bounded, all-valid fixture for the external readers'
        expected = digest(original)
        directory = output/f'{index}-{original.stem}'
        directory.mkdir()
        isolated = directory/'input.e57'
        shutil.copyfile(original, isolated)  # Exclude unrelated photo/edit sidecars.
        destination = directory/'roundtrip.e57'
        run([str(args.binary.resolve()), '--section', str(isolated),
             '-1000000000,-1000000000,-1000000000,1000000000,1000000000,1000000000', str(destination)],
            directory/'native.log', env)
        checked = compare_libe57(isolated, destination)
        case = {'source': original.name, 'source_points': count, 'source_sha256': expected,
                'libe57': checked, 'pdal': {}, 'cloudcompare': {}}
        for reader in ['pdal', 'cloudcompare']:
            texts = []
            for label, path in [('original', isolated), ('roundtrip', destination)]:
                text = directory/f'{reader}-{label}.txt'
                log = directory/f'{reader}-{label}.log'
                if reader == 'pdal':
                    command = [str(args.pdal.resolve()), 'translate', str(path), str(text), '--writers.text.precision=17']
                else:
                    command = [str(args.cloudcompare.resolve()), '-SILENT', '-AUTO_SAVE', 'OFF',
                               '-O', '-GLOBAL_SHIFT', 'AUTO', str(path), '-C_EXPORT_FMT', 'ASC',
                               '-PREC', '16', '-ADD_HEADER', '-NO_TIMESTAMP']
                    if source.scan_count > 1:
                        command += ['-MERGE_CLOUDS']
                    command += ['-SAVE_CLOUDS', 'FILE', str(text)]
                run(command, log, env)
                if not text.exists() and reader == 'cloudcompare':
                    alternatives = sorted(directory.glob(text.name+'_*'))
                    assert len(alternatives) == 1, 'CloudCompare must produce one merged point file'
                    text = alternatives[0]
                assert text_rows(text) == count, 'External reader changed the point count'
                if reader == 'cloudcompare':
                    messages = log.read_text(errors='replace')
                    assert 'Failed to load image' not in messages, 'Qt image plugins must be available'
                    image_count = len(re.findall(r'Reading new image node', messages))
                    assert image_count == checked['images'], 'CloudCompare did not read all photographs'
                    case[reader]['images_read'] = image_count
                    case[reader]['unhandled_camera_types'] = messages.count('Unhandled camera type')
                texts.append(text)
            assert digest(texts[0]) == digest(texts[1]), f'{reader} values differ before and after export'
            case[reader]['points'] = count
            case[reader]['normalised_text_sha256'] = digest(texts[0])
            case[reader]['before_after_identical'] = True
        assert digest(isolated) == expected, 'Isolated input bytes changed'
        assert digest(original) == expected, 'Original fixture bytes changed'
        case['source_unchanged'] = True
        report['cases'].append(case)
        (output/'result.json').write_text(json.dumps(report, indent=2)+'\n')
        print(f'{original.name}: {count} points, {checked["images"]} photos passed all external readers', flush=True)


if __name__ == '__main__':
    main()
