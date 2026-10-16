"""Verify x64 PE imports in an owned Windows package/installation directory."""
import argparse
import json
import os
from pathlib import Path
import re
import struct


def pe_imports(path, require_x64=True):
    data = path.read_bytes()
    pe = struct.unpack_from('<I', data, 0x3c)[0]
    if data[:2] != b'MZ' or data[pe:pe + 4] != b'PE\0\0':
        raise ValueError(f'Invalid PE: {path.name}')
    machine, count = struct.unpack_from('<HH', data, pe + 4)
    optional = pe + 24
    optional_size = struct.unpack_from('<H', data, pe + 20)[0]
    magic = struct.unpack_from('<H', data, optional)[0]
    if (machine, magic) not in [(0x8664, 0x20b), (0x14c, 0x10b)]:
        raise ValueError(f'Unsupported PE architecture: {path.name}')
    if require_x64 and machine != 0x8664:
        raise ValueError(f'Expected Windows x64 binary: {path.name}')
    directories = optional + (112 if magic == 0x20b else 96)
    sections = []
    for index in range(count):
        start = optional + optional_size + index * 40
        virtual_size, virtual, raw_size, raw = struct.unpack_from('<IIII', data, start + 8)
        sections.append((virtual, max(virtual_size, raw_size), raw))
    headers_size = struct.unpack_from('<I', data, optional + 60)[0]

    def offset(rva):
        if rva < headers_size:
            return rva
        for virtual, size, raw in sections:
            if virtual <= rva < virtual + size:
                return raw + rva - virtual
        raise ValueError(f'Invalid import RVA in {path.name}')

    def name(rva):
        start = offset(rva)
        value = data[start:data.index(0, start)].decode('ascii').lower()
        if not re.fullmatch(r'[a-z0-9_.-]+\.dll', value):
            raise ValueError(f'Invalid import name in {path.name}: {value!r}')
        return value

    imports = set()
    for directory, width, name_field in [(1, 20, 12), (13, 32, 4)]:
        if directory >= struct.unpack_from('<I', data, directories - 4)[0]:
            continue
        rva = struct.unpack_from('<I', data, directories + directory * 8)[0]
        if not rva:
            continue
        start = offset(rva)
        for index in range(4096):
            position = start + index * width
            descriptor = data[position:position + width]
            if len(descriptor) != width:
                raise ValueError(f'Truncated import table in {path.name}')
            if not any(descriptor):
                break
            if directory == 13 and struct.unpack_from('<I', descriptor)[0] != 1:
                raise ValueError(f'Unsupported delay import address in {path.name}')
            imports.add(name(struct.unpack_from('<I', descriptor, name_field)[0]))
        else:
            raise ValueError(f'Unbounded import table in {path.name}')
    return sorted(imports)


def verify(root, system):
    binaries = sorted(p for p in root.rglob('*') if p.suffix.lower() in ['.exe', '.dll'])
    if not binaries:
        raise ValueError('Package has no Windows binaries')
    result = []
    for binary in binaries:
        resolved = {}
        # NSIS's small uninstaller is 32-bit even in an x64 application bundle.
        require_x64 = not binary.name.lower().startswith('uninstall')
        for dependency in pe_imports(binary, require_x64=require_x64):
            if dependency.startswith(('api-ms-win-', 'ext-ms-win-')):
                resolved[dependency] = 'Windows API set'
            elif (binary.parent / dependency).is_file() or (root / dependency).is_file():
                resolved[dependency] = 'packaged'
            elif re.match(r'(vcruntime|msvcp|msvcr|concrt|vcomp|mfc|mfcm)\d', dependency):
                raise ValueError(f'{binary.name} requires an unpackaged redistributable: {dependency}')
            elif (system / dependency).is_file():
                resolved[dependency] = 'Windows system'
            else:
                raise ValueError(f'{binary.name} has an unresolved import: {dependency}')
        result.append({'path': binary.relative_to(root).as_posix(), 'imports': resolved})
    return {'passed': True, 'binaries': result}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('root', type=Path)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    if os.name != 'nt':
        raise SystemExit('Import resolution must run on Windows')
    result = verify(args.root.resolve(strict=True), Path(os.environ['SystemRoot']) / 'System32')
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(result, indent=2))
    print(json.dumps({'windows_dependencies': 'passed', 'binaries': len(result['binaries'])}))
