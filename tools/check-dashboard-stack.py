#!/usr/bin/env python3
"""Reject oversized Rust stack frames in the actual ESP32-C3 release ELF."""
import json
from pathlib import Path
import re
import subprocess
import sys

MAX_FRAME = 24 * 1024
NESTED_RESERVE = 16 * 1024


def check(path):
    header = Path(path).read_bytes()[:20]
    if header[:6] != b'\x7fELF\x01\x01' or int.from_bytes(header[18:20], 'little') != 243:
        raise ValueError('expected a little-endian, 32-bit RISC-V ELF')
    symbols = subprocess.check_output(['readelf', '--wide', '--symbols', str(path)], text=True)
    stack = {}
    for line in symbols.splitlines():
        columns = line.split()
        if len(columns) >= 8 and columns[-1] in ('_stack_start_cpu0', '_stack_end_cpu0'):
            stack[columns[-1]] = int(columns[1], 16)
    capacity = stack['_stack_start_cpu0'] - stack['_stack_end_cpu0']
    unwind = subprocess.check_output(['readelf', '--debug-dump=frames', str(path)], text=True)
    frames, address, size = {}, None, 0
    for line in unwind.splitlines():
        if ' FDE ' in line:
            if address is not None:
                frames[address] = max(frames.get(address, 0), size)
            match = re.search(r'pc=([0-9a-fA-F]+)\.\.', line)
            address, size = (int(match[1], 16) if match else None), 0
        match = re.search(r'DW_CFA_def_cfa_offset(?:_sf)?: (\d+)', line)
        if match:
            size = max(size, int(match[1]))
    if address is not None:
        frames[address] = max(frames.get(address, 0), size)
    if not frames or not max(frames.values()):
        raise ValueError('release ELF needs stack unwind information for this check')
    largest = max(frames.values())
    report = {'cpu_stack_bytes': capacity, 'largest_frame_bytes': largest,
              'maximum_frame_bytes': MAX_FRAME, 'nested_reserve_bytes': NESTED_RESERVE,
              'frames_checked': len(frames)}
    print(json.dumps(report))
    if largest > MAX_FRAME or capacity - largest < NESTED_RESERVE:
        raise ValueError('oversized stack frame or insufficient room for nested calls')


if __name__ == '__main__':
    try:
        check(sys.argv[1])
    except (IndexError, KeyError, OSError, ValueError, subprocess.CalledProcessError) as error:
        sys.exit(f'Stack check failed: {error}')
