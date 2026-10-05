#!/usr/bin/env python3
"""Resolve DRT sizing probe (openfx-vertical-mismatch-from-drt).

Checks, on the machine it runs on, the platform pieces the OpenFX plugin relies on to read the
vertical mismatched-resolution mode of a vertical Resolve timeline:

  - fuscript runs Lua with bmd.gettime;
  - Timeline:Export(path, resolve.EXPORT_DRT) writes into this user's temp directory, with the path
    embedded as a pure-ASCII Lua literal (decimal escapes, as the plugin does);
  - the file is readable and deletable from the reading side;
  - the DRT SequenceSetup validates against the scripting API readings (design D5) and decodes
    @96 / @112 / the effective mode;
  - a line flushed before fuscript is killed still reaches the pipe (the gf_drt_begin marker).

It only exports the current timeline and never changes a setting. The export goes to the temp
directory and is deleted afterwards. Resolve may add an entry to the timeline's backup list.
Run it with the test project open and a vertical timeline current:

  python3 probe.py --project "<current project name>" [--expect-effective scaleToCrop]

Exit status 0 when every check passes. Standard library only.
"""
import argparse
import io
import os
import re
import struct
import subprocess
import sys
import tempfile
import time
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path

FUSCRIPT_CANDIDATES = (
    Path('/Applications/DaVinci Resolve/DaVinci Resolve.app/Contents/Libraries/Fusion/fuscript'),
    Path('C:/Program Files/Blackmagic Design/DaVinci Resolve/fuscript.exe'),
)
MODES = {-1: 'scaleToFit', 0: 'centerCrop', 1: 'scaleToCrop', 2: 'stretch'}
QVARIANT_MAX_DEPTH = 32
SETUP_MIN_LEN = 116
MARKER = 'gf_drt_begin=1'

# Mirrors the plugin's core query (fuscript.rs CORE_QUERY_LUA) for the setting reads, then exports
# the current timeline. Every value is printed as `key=value`; names travel hex-encoded.
EXPORT_LUA = r"""
local function hex(s) return (s:gsub('.', function(c) return string.format('%02x', c:byte()) end)) end
local r = Resolve()
local proj = r:GetProjectManager():GetCurrentProject()
print('project_hex=' .. hex(proj:GetName() or ''))
if hex(proj:GetName() or '') ~= EXPECT_PROJECT_HEX then print('guard=project-mismatch'); print('end=1'); return end
local tl = proj:GetCurrentTimeline()
print('timeline_hex=' .. hex(tl:GetName() or ''))
local ucs = tl:GetSetting('useCustomSettings') or ''
local mm
if ucs == '1' then mm = tl:GetSetting('timelineInputResMismatchBehavior') or '' else mm = proj:GetSetting('timelineInputResMismatchBehavior') or '' end
local tw, th = '', ''
if ucs == '1' then tw = tl:GetSetting('timelineResolutionWidth') or ''; th = tl:GetSetting('timelineResolutionHeight') or '' end
if tw == '' or th == '' then tw = proj:GetSetting('timelineResolutionWidth') or ''; th = proj:GetSetting('timelineResolutionHeight') or '' end
print('ucs=' .. ucs); print('mm=' .. mm); print('tw=' .. tw); print('th=' .. th)
print('gettime=' .. tostring(bmd ~= nil and bmd.gettime ~= nil))
print('export_drt=' .. tostring(r.EXPORT_DRT))
local ok, err = pcall(function()
    local t0 = bmd.gettime()
    local done = tl:Export(DRT_PATH, r.EXPORT_DRT)
    print(string.format('export_ms=%.3f', (bmd.gettime() - t0) * 1000))
    print('export_ok=' .. tostring(done))
end)
if not ok then print('export_error=' .. (string.gsub(tostring(err), '[%c]', ' '))) end
print('end=1')
"""

MARKER_LUA = (
    "local r = Resolve()\n"
    "print('gf_probe_before=1')\n"
    "print('" + MARKER + "')\n"
    "if io ~= nil and io.flush ~= nil then io.flush() end\n"
    "bmd.wait(10)\n"
    "print('gf_probe_after=1')\n"
)


def lua_ascii_literal(text):
    """Single-quoted Lua literal; bytes outside 0x20..0x7E plus backslash and quote as 3-digit escapes."""
    out = []
    for b in text.encode('utf-8'):
        if 0x20 <= b <= 0x7E and b not in (0x5C, 0x27):
            out.append(chr(b))
        else:
            out.append('\\%03d' % b)
    return "'" + ''.join(out) + "'"


def find_fuscript():
    for path in FUSCRIPT_CANDIDATES:
        if path.exists():
            return path
    raise SystemExit('fuscript not found in: ' + ', '.join(str(p) for p in FUSCRIPT_CANDIDATES))


def run_fuscript(fuscript, script, timeout):
    proc = subprocess.run([str(fuscript), '-q', '-l', 'lua', '-x', script], cwd=str(fuscript.parent),
                          capture_output=True, timeout=timeout,
                          creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
    values = {}
    for line in proc.stdout.decode('utf-8', 'replace').splitlines():
        key, sep, value = line.strip().partition('=')
        if sep:
            values[key] = value
    return values, proc.stderr.decode('utf-8', 'replace').strip()


# ---------------------------------------------------------------------------------------------
# DRT parsing and validation (design D5, same rules as openfx/src/drt_sizing.rs)
# ---------------------------------------------------------------------------------------------

class Rejection(Exception):
    pass


class Reader:
    def __init__(self, data):
        self.data, self.pos = data, 0

    def take(self, n):
        if n < 0 or self.pos + n > len(self.data):
            raise Rejection('blob-decode(truncated)')
        chunk = self.data[self.pos:self.pos + n]
        self.pos += n
        return chunk

    def u32(self):
        return struct.unpack('>I', self.take(4))[0]

    def string(self):
        n = self.u32()
        if n == 0xFFFFFFFF:
            return ''
        if n % 2:
            raise Rejection('blob-decode(odd QString length)')
        return self.take(n).decode('utf-16-be', 'replace')

    def bytearray(self):
        n = self.u32()
        return b'' if n == 0xFFFFFFFF else self.take(n)

    def variant(self, depth=0):
        if depth > QVARIANT_MAX_DEPTH:
            raise Rejection('blob-decode(QVariant nesting exceeds %d levels)' % QVARIANT_MAX_DEPTH)
        kind = self.u32()
        self.take(1)  # null flag
        if kind == 1:
            self.take(1)
        elif kind in (2, 3):
            self.take(4)
        elif kind in (4, 5, 6):
            self.take(8)
        elif kind == 8:
            for _ in range(self.u32()):
                self.string()
                self.variant(depth + 1)
        elif kind == 9:
            for _ in range(self.u32()):
                self.variant(depth + 1)
        elif kind == 10:
            return self.string()
        elif kind == 11:
            for _ in range(self.u32()):
                self.string()
        elif kind == 12:
            return self.bytearray()
        else:
            raise Rejection('blob-decode(unknown QVariant type %d)' % kind)
        return None


def sequence_setup(drt_bytes, timeline_name):
    try:
        xml = zipfile.ZipFile(io.BytesIO(drt_bytes)).read('MediaPool/Master/MpFolder.xml').decode('utf-8')
        root = ET.fromstring(re.sub(r'(</?)([A-Za-z0-9_]+)::', r'\1\2__', xml))
    except Exception as exc:  # zip, encoding or XML failure
        raise Rejection('zip(%s)' % exc)
    matches = [t for t in root.iter('Sm2Timeline')
               if t.find('Name') is not None and (t.find('Name').text or '') == timeline_name]
    if not matches:
        raise Rejection('timeline-not-found')
    if len(matches) > 1:
        raise Rejection('timeline-ambiguous(%d)' % len(matches))
    blob = matches[0].find('Sequence/Sm2Sequence/FieldsBlob')
    if blob is None or not (blob.text or '').strip():
        raise Rejection('setup-missing')
    try:
        data = bytes.fromhex(blob.text.strip())
    except ValueError:
        raise Rejection('blob-decode(invalid hex)')
    reader = Reader(data)
    reader.u32()  # QDataStream version
    setup = None
    for _ in range(reader.u32()):
        key = reader.string()
        value = reader.variant()
        if key == 'SequenceSetup':
            setup = value
    if not isinstance(setup, (bytes, bytearray)):
        raise Rejection('setup-missing')
    if len(setup) < SETUP_MIN_LEN:
        raise Rejection('setup-too-short(%d)' % len(setup))
    return setup


def validate(setup, width, height, api_mode):
    word = lambda offset: struct.unpack('>i', setup[offset:offset + 4])[0]
    w, h, horizontal, vertical = word(44), word(52), word(96), word(112)
    if (w, h) != (width, height):
        raise Rejection('resolution-mismatch(drt=%dx%d api=%dx%d)' % (w, h, width, height))
    if MODES.get(horizontal) != api_mode:
        raise Rejection('horizontal-mismatch(drt=%d api=%s)' % (horizontal, api_mode))
    if vertical not in MODES:
        raise Rejection('vertical-out-of-range(%d)' % vertical)
    if not h > w:
        raise Rejection('not-vertical')
    return horizontal, vertical, MODES[vertical]


# ---------------------------------------------------------------------------------------------
# Checks
# ---------------------------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--project', required=True, help='name of the currently open project (guard: nothing is exported otherwise)')
    parser.add_argument('--expect-effective', choices=sorted(MODES.values()), help='also require this effective mode')
    args = parser.parse_args()

    results = []

    def report(ok, text):
        results.append(ok)
        print(('PASS  ' if ok else 'FAIL  ') + text)

    fuscript = find_fuscript()
    temp_dir = Path(tempfile.gettempdir())
    drt_path = temp_dir / ('gyroflow-drt-probe-%d.drt' % os.getpid())
    print('INFO  fuscript: %s' % fuscript)
    print('INFO  temp dir: %s' % temp_dir)

    script = ('EXPECT_PROJECT_HEX = ' + lua_ascii_literal(args.project.encode('utf-8').hex()) + '\n'
              + 'DRT_PATH = ' + lua_ascii_literal(str(drt_path)) + '\n' + EXPORT_LUA)
    try:
        values, stderr = run_fuscript(fuscript, script, timeout=60)
    except subprocess.TimeoutExpired:
        raise SystemExit('fuscript did not answer within 60 s; is Resolve running with external scripting set to Local?')
    if values.get('guard') == 'project-mismatch':
        current = bytes.fromhex(values.get('project_hex', '')).decode('utf-8', 'replace')
        raise SystemExit('guard: the current project is %r, not %r; nothing was exported' % (current, args.project))
    if values.get('end') != '1':
        raise SystemExit('the Lua script did not finish.\nstdout values: %r\nstderr: %s' % (values, stderr))

    timeline = bytes.fromhex(values.get('timeline_hex', '')).decode('utf-8', 'replace')
    api_mode, tw, th = values.get('mm', ''), values.get('tw', ''), values.get('th', '')
    print('INFO  timeline %r: useCustomSettings=%s api mode=%s resolution=%sx%s' % (timeline, values.get('ucs'), api_mode, tw, th))

    report(values.get('gettime') == 'true', 'bmd.gettime present')
    export_ok = values.get('export_ok') == 'true' and drt_path.exists()
    report(export_ok, 'export ok (%s ms, resolve.EXPORT_DRT=%s)%s' % (
        values.get('export_ms', '?'), values.get('export_drt'),
        '' if export_ok else ' error=%s stderr=%s' % (values.get('export_error'), stderr)))

    data = None
    try:
        data = drt_path.read_bytes()
        report(True, 'file readable (%d bytes) at %s' % (len(data), drt_path))
    except OSError as exc:
        report(False, 'file readable: %s' % exc)
    finally:
        try:
            drt_path.unlink()
        except FileNotFoundError:
            pass
        except OSError as exc:
            print('INFO  delete failed: %s' % exc)
        report(not drt_path.exists(), 'file deleted')

    if data is not None:
        try:
            width, height = int(tw), int(th)
        except ValueError:
            width = height = -1
        try:
            started = time.perf_counter()
            setup = sequence_setup(data, timeline)
            horizontal, vertical, effective = validate(setup, width, height, api_mode)
            report(True, 'validation (parse %.1f ms)' % ((time.perf_counter() - started) * 1000))
            report(True, 'decoded @96=%d (%s) @112=%d (%s) effective=%s' % (
                horizontal, MODES[horizontal], vertical, MODES[vertical], effective))
            if args.expect_effective:
                report(effective == args.expect_effective, 'effective is %s (expected %s)' % (effective, args.expect_effective))
        except Rejection as rejection:
            report(False, 'validation: %s' % rejection)

    # The plugin charges a killed query only when this flushed marker reached the pipe.
    proc = subprocess.Popen([str(fuscript), '-q', '-l', 'lua', '-x', MARKER_LUA], cwd=str(fuscript.parent),
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
    time.sleep(2.0)
    still_running = proc.poll() is None
    proc.kill()
    out, _ = proc.communicate(timeout=10)
    seen = MARKER in out.decode('utf-8', 'replace').splitlines()
    report(still_running and seen, 'flushed marker survives the kill (running at kill=%s, marker seen=%s)' % (still_running, seen))

    print('ALL PASS' if all(results) else 'SOME CHECKS FAILED')
    return 0 if all(results) else 1


if __name__ == '__main__':
    sys.exit(main())
