"""Project-owned native and WASM dependency records for real image inventory tests."""

import json
import struct
import zlib


def leb(value: int) -> bytes:
    result = bytearray()
    while value > 127:
        result.append((value & 127) | 128)
        value >>= 7
    result.append(value)
    return bytes(result)


def audit(name: str) -> bytes:
    return zlib.compress(
        json.dumps(
            {
                'packages': [
                    {
                        'name': name,
                        'version': '0.1.0',
                        'source': 'local',
                        'root': True,
                        'dependencies': [1],
                    },
                    {
                        'name': 'synthetic-dependency',
                        'version': '1.2.3',
                        'source': 'crates.io',
                        'dependencies': [],
                    },
                ]
            }
        ).encode()
    )


def wasm(name: str = 'garmin-hass-web') -> bytes:
    section = b'\x07.dep-v0' + audit(name)
    return b'\0asm\x01\0\0\0\0' + leb(len(section)) + section


def elf(name: str, machine: int = 62) -> bytes:
    strings = b'\0.shstrtab\0.dep-v0\0'
    data = audit(name)
    offset = 64 + 3 * 64
    header = b'\x7fELF\x02\x01\x01' + b'\0' * 9
    header += struct.pack('<HHIQQQIHHHHHH', 2, machine, 1, 0, 0, 64, 0, 64, 0, 0, 64, 3, 1)
    sections = b'\0' * 64
    sections += struct.pack('<IIQQQQIIQQ', 1, 3, 0, 0, offset, len(strings), 0, 0, 1, 0)
    sections += struct.pack('<IIQQQQIIQQ', 11, 1, 0, 0, offset + len(strings), len(data), 0, 0, 1, 0)
    return header + sections + strings + data
