#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Compare pre-rotated inputs with raw-pixel references through the real OFX entry.

Requires GYROFLOW_ROTATION_CORE and GYROFLOW_ROTATION_PROJECT. An optional
GYROFLOW_ROTATION_BASELINE_CORE verifies Resolve against the previous binary.
"""
import ctypes as C
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def render_cases(core, fixture, project, output, host):
    import numpy as np
    output.mkdir(parents=True, exist_ok=True)
    os.environ['GYROFLOW_DATA_DIR'] = str(output/'data')
    lib = C.CDLL(str(fixture))
    declarations = {
        'fixture_set_host_name': ([C.c_char_p], None),
        'fixture_open': ([C.c_wchar_p, C.c_wchar_p, C.c_int, C.c_int], C.c_int),
        'fixture_new_instance': ([], C.c_void_p),
        'fixture_create': ([C.c_void_p], C.c_int),
        'fixture_import_value': ([C.c_void_p, C.c_char_p, C.c_int, C.c_double, C.c_char_p], C.c_int),
        'fixture_notify': ([C.c_void_p, C.c_char_p, C.c_char_p, C.c_double], C.c_int),
        'fixture_action': ([C.c_void_p, C.c_char_p, C.c_char_p, C.c_double], C.c_int),
        'fixture_snapshot': ([C.c_void_p], C.c_char_p),
        'fixture_rotation_images': ([C.c_void_p, C.c_int, C.c_int, C.c_int, C.c_int, C.c_double, C.c_void_p], None),
        'fixture_rotation_render': ([C.c_void_p, C.c_double, C.c_double], C.c_int),
        'fixture_output_data': ([C.c_void_p, C.POINTER(C.c_size_t)], C.c_void_p),
        'fixture_close': ([], None),
    }
    for name, (arguments, result) in declarations.items():
        function = getattr(lib, name)
        function.argtypes, function.restype = arguments, result
    lib.fixture_set_host_name(host.encode())
    assert lib.fixture_open(str(core), str(output/'trace.jsonl'), int(host == 'com.lemon.lvpro'), 1) == 0
    original = project.read_bytes()
    data = json.loads(original)
    width, height = data['video_info']['width'], data['video_info']['height']
    assert width > height
    report = {'core_sha256': hashlib.sha256(core.read_bytes()).hexdigest(), 'host': host, 'cases': {}}

    def snapshot(instance):
        return json.loads(lib.fixture_snapshot(instance).decode())

    def set_value(instance, name, value):
        kind = snapshot(instance)['parameters'][name]['type']
        text = str(value).encode() if kind == 'OfxParamTypeString' else None
        tag = 2 if text is not None else 1 if kind == 'OfxParamTypeDouble' else 0
        assert lib.fixture_import_value(instance, name.encode(), tag, 0 if text is not None else value, text) == 0
        assert lib.fixture_notify(instance, name.encode(), b'OfxChangeUserEdited', 0) == 0

    def action(instance, name):
        status = lib.fixture_action(instance, name.encode(), b'OfxChangeUserEdited', 0)
        assert status in ([0, 14] if host != 'com.lemon.lvpro' else [0]), (name, status)

    try:
        for rotation in [0, 90, 180, 270]:
            side = rotation in [90, 270]
            data['video_info']['rotation'] = rotation
            data['output']['output_width'] = height if side else width
            data['output']['output_height'] = width if side else height
            derived = output/f'rotation-{rotation}.gyroflow'
            derived.write_text(json.dumps(data), encoding='utf-8')
            for scale, frame, fit in [(0.25, 60, False), (0.5, 180, False), (0.25, 60, True)]:
                w, h = round(width*scale), round(height*scale)
                y, x = np.indices((h, w))
                raw = np.empty((h, w, 4), dtype=np.uint8)
                raw[..., 0] = 128 + 45*np.sin(x/13) + 30*np.cos(y/17)
                raw[..., 1] = 128 + 43*np.sin(x/19+y/23)
                raw[..., 2] = 128 + 40*np.cos(x/29-y/11)
                raw[..., 3] = 255
                rotated = np.ascontiguousarray(np.rot90(raw, rotation//90))
                out_w, out_h = (h, w) if side else (w, h)
                if fit:
                    extent = max(w, h)
                    out_w = out_h = extent
                images = {}
                values = {}
                for label, pixels, index in [('raw', raw, 0), ('host', rotated, {0: 0, 90: 1, 180: 3, 270: 2}[rotation])]:
                    if fit:
                        canvas = np.zeros((extent, extent, 4), dtype=np.uint8)
                        px, py = (extent-pixels.shape[1])//2, (extent-pixels.shape[0])//2
                        canvas[py:py+pixels.shape[0], px:px+pixels.shape[1]] = pixels
                        pixels = canvas
                    instance = lib.fixture_new_instance()
                    lib.fixture_rotation_images(instance, pixels.shape[1], pixels.shape[0], out_w, out_h, scale, pixels.ctypes.data)
                    assert lib.fixture_create(instance) == 0
                    action(instance, 'OfxActionBeginInstanceChanged')
                    set_value(instance, 'ProjectPath', str(derived))
                    set_value(instance, 'ReloadProject', 1)
                    action(instance, 'OfxActionEndInstanceChanged')
                    action(instance, 'OfxActionBeginInstanceChanged')
                    set_value(instance, 'InputRotation', index)
                    set_value(instance, 'TranslationEnabled', 0)
                    set_value(instance, 'HostInputSizing', 1)
                    action(instance, 'OfxActionEndInstanceChanged')
                    time = frame/data['video_info']['fps']*30
                    assert lib.fixture_rotation_render(instance, time, scale) == 0
                    count = C.c_size_t()
                    address = lib.fixture_output_data(instance, C.byref(count))
                    image = C.string_at(address, count.value)
                    assert len(image) == out_w*out_h*4
                    images[label] = np.frombuffer(image, dtype=np.uint8).astype(np.int16)
                    key = f'{rotation}-{scale}-{fit}-{label}'
                    (output/(key+'.rgba')).write_bytes(image)
                    values[label] = snapshot(instance)
                difference = abs(images['raw']-images['host'])
                key = f'{rotation}-{scale}-{fit}'
                report['cases'][key] = {
                    'mae': float(difference.mean()), 'max_error': int(difference.max()),
                    'raw_hash': hashlib.sha256(images['raw'].astype(np.uint8)).hexdigest(),
                    'host_hash': hashlib.sha256(images['host'].astype(np.uint8)).hexdigest(),
                    'forbidden_writes': [values[label]['forbidden_writes'] for label in ['raw', 'host']],
                    'parameters': {name: values['host']['parameters'][name]['value'] for name in ['InputRotation', 'Rotation', 'OutputWidth', 'OutputHeight']},
                }
    finally:
        lib.fixture_close()
    assert project.read_bytes() == original
    (output/'result.json').write_text(json.dumps(report, indent=2)+'\n', encoding='utf-8')


@unittest.skipUnless(sys.platform == 'win32' and os.environ.get('GYROFLOW_ROTATION_CORE') and os.environ.get('GYROFLOW_ROTATION_PROJECT'),
                     'requires an OFX DLL and a read-only landscape-storage project')
class OpenfxInputRotationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory(prefix='ofx-rotation-')
        cls.output = Path(os.environ.get('GYROFLOW_ROTATION_OUTPUT', cls.temporary.name)).resolve()
        cls.output.mkdir(parents=True, exist_ok=True)
        sys.path.insert(0, str(ROOT/'scripts'))
        import build_jianying_package as build
        environment = build.msvc_environment()
        fixture = cls.output/'rotation-host.dll'
        subprocess.run([shutil.which('cl', path=environment['PATH']), '/nologo', '/std:c++17', '/EHsc', '/O2', '/utf-8', '/LD',
                        str(ROOT/'tests/fixtures/openfx_rotation_host.cpp'), '/I', str(build.openfx_headers(environment)),
                        f'/Fo:{cls.output / "host.obj"}', f'/Fe:{fixture}'], cwd=cls.output, env=environment, check=True, capture_output=True)
        cls.results = {}
        cores = {'current': Path(os.environ['GYROFLOW_ROTATION_CORE']).resolve()}
        if os.environ.get('GYROFLOW_ROTATION_BASELINE_CORE'):
            cores['baseline'] = Path(os.environ['GYROFLOW_ROTATION_BASELINE_CORE']).resolve()
        for version, core in cores.items():
            for host in ['com.lemon.lvpro', 'DaVinciResolve']:
                target = cls.output/(version+'-'+host)
                command = [sys.executable, str(Path(__file__).resolve()), '--render', str(core), str(fixture),
                           str(Path(os.environ['GYROFLOW_ROTATION_PROJECT']).resolve()), str(target), host]
                result = subprocess.run(command, capture_output=True, text=True, encoding='utf-8', errors='replace')
                (cls.output/(version+'-'+host+'.log')).write_text(result.stdout+result.stderr, encoding='utf-8')
                if result.returncode:
                    raise AssertionError(result.stdout+result.stderr)
                cls.results[version, host] = json.loads((target/'result.json').read_text())

    @classmethod
    def tearDownClass(cls):
        cls.temporary.cleanup()

    def test_jianying_matches_raw_pixel_reference(self):
        for key, row in self.results['current', 'com.lemon.lvpro']['cases'].items():
            with self.subTest(case=key):
                self.assertLessEqual(row['mae'], 0.001)
                self.assertLessEqual(row['max_error'], 1)
                self.assertEqual(row['forbidden_writes'], [0, 0])

    def test_resolve_pixels_are_unchanged(self):
        if ('baseline', 'DaVinciResolve') not in self.results:
            self.skipTest('set GYROFLOW_ROTATION_BASELINE_CORE for a binary comparison')
        current = self.results['current', 'DaVinciResolve']['cases']
        baseline = self.results['baseline', 'DaVinciResolve']['cases']
        for key, row in current.items():
            with self.subTest(case=key):
                for field in ['raw_hash', 'host_hash', 'parameters', 'forbidden_writes']:
                    self.assertEqual(row[field], baseline[key][field])


if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == '--render':
        render_cases(*(Path(value).resolve() for value in sys.argv[2:6]), sys.argv[6])
    else:
        unittest.main()
