"""Add GD 2.2 objects missing from data/objects.json (ids GD's own table has).

GD fills its id -> frame table in ObjectToolbox::init: per entry
`mov [rbp+X], id; call <map index>; lea rdx, "frame.png"; call <assign>`.
This reads those pairs out of an objdump of the Windows exe and adds every
id that is not in objects.json yet, copying channel/z defaults from an
existing object of the same sprite family (decoration fallback otherwise).

shortcut: new ids are decoration (no hitbox) except dash orbs; 2.2 blocks and
spikes (gdh_*) are therefore not solid in a pure sim run. Replays follow the
recorded path, so only the pure sim is affected; add hitboxes if that matters.

usage: objdump -d -M intel --no-show-raw-insn GeometryDash.exe > exe.asm
       python3 -I tools/gd_frames.py GeometryDash.exe exe.asm data/objects.json
"""
import json
import os
import re
import sys

MAP_INDEX = 'call   0x1402d37f0'  # std::map<int, string>::operator[] in this exe
RDATA_VA, RDATA_OFF, RDATA_LEN = 0x1406CA000, 0x6C8600, 0x22D794
SHEET_DIR = os.environ.get('GD_RESOURCES') or sys.exit('set GD_RESOURCES to Geometry Dash\'s Resources folder')


def frames(exe_path, asm_path):
    exe = open(exe_path, 'rb').read()

    def cstr(va):
        off = va - RDATA_VA + RDATA_OFF
        if not RDATA_OFF <= off < RDATA_OFF + RDATA_LEN:
            return None
        return exe[off:exe.index(b'\0', off)].decode('latin1')

    out, imm, pending = {}, None, None
    for line in open(asm_path):
        m = re.search(r'mov\s+DWORD PTR \[rbp[+-]0x[0-9a-f]+\],(0x[0-9a-f]+)$', line.rstrip())
        if m:
            imm = int(m.group(1), 16)
        elif MAP_INDEX in line:
            pending = imm
        elif pending is not None and (m := re.search(r'lea\s+rdx,\[rip\+0x[0-9a-f]+\]\s+# (0x[0-9a-f]+)', line)):
            s = cstr(int(m.group(1), 16))
            if s and s.endswith('.png'):
                out.setdefault(pending, s)
            pending = None
    return out


def family(name):
    return re.sub(r'(_[0-9a-z]+)*_00\d(\.png)?$', '', re.sub(r'_\d.*', '', name))


def main(exe_path, asm_path, table_path):
    import glob
    tab = json.load(open(table_path))
    pairs = frames(exe_path, asm_path)
    # sanity: the extraction must agree with the table where both know an id
    both = [k for k in pairs if str(k) in tab and tab[str(k)].get('texture_name')]
    agree = sum(pairs[k].removesuffix('.png') == tab[str(k)]['texture_name'].removesuffix('.png') for k in both)
    assert agree >= 0.99 * len(both), f'only {agree}/{len(both)} agree: wrong function/exe?'
    sheets = set()
    for p in glob.glob(SHEET_DIR + '/*-uhd.plist'):
        sheets |= set(re.findall(r'<key>([^<]+\.png)</key>', open(p, errors='ignore').read()))
    fams = {}
    for v in tab.values():
        if v.get('texture_name') and 'hitbox' not in v:
            fams.setdefault(family(v['texture_name']), v)
    deco = {'color_channel': 'base', 'default_primary_channel': 1004, 'default_secondary_channel': -1,
            'default_z_layer': 1, 'default_z_order': -7, 'object_type': 7}
    orb = {k: v for k, v in tab['36'].items() if k != 'texture_name'}  # yellow orb: dash orb hitbox
    added = 0
    for k, frame in sorted(pairs.items()):
        if str(k) in tab or frame.startswith('edit_') or frame not in sheets:
            continue  # known, editor-only (triggers), or not drawable
        base = orb if frame.startswith('dashRing') else fams.get(family(frame), deco)
        e = {f: v for f, v in base.items() if f not in ('texture_name', 'children')}
        e['texture_name'] = frame.removesuffix('.png')
        tab[str(k)] = e
        added += 1
    json.dump(tab, open(table_path, 'w'), indent=1)
    print(f'{len(pairs)} ids in GD, {agree}/{len(both)} agree with the table, added {added}')


if __name__ == '__main__':
    main(*sys.argv[1:4])
