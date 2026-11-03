"""
Builds assets/lumen.res (the exe icon) from assets/lumen.ico, without the Windows SDK's rc.exe.

A .res file is a list of resources, each with a small header. The icon becomes one RT_ICON per
size plus an RT_GROUP_ICON that lists them; the MSVC linker takes the .res as-is (see build.rs).

    python assets/make_res.py
"""
import os
import struct

RT_ICON, RT_GROUP_ICON = 3, 14


def resource(rtype, name, data, flags=0x1010, lang=0x0409):
    header = struct.pack("<HHHH", 0xFFFF, rtype, 0xFFFF, name)
    header += struct.pack("<IHHII", 0, flags, lang, 0, 0)
    size = 8 + len(header)
    out = struct.pack("<II", len(data), size) + header + data
    return out + b"\0" * (-len(out) % 4)


def main():
    here = os.path.dirname(os.path.abspath(__file__))
    ico = open(os.path.join(here, "lumen.ico"), "rb").read()
    _, _, count = struct.unpack_from("<HHH", ico, 0)
    res = resource(0, 0, b"", flags=0, lang=0)  # the empty first entry every .res starts with
    group = struct.pack("<HHH", 0, 1, count)
    for i in range(count):
        w, h, colors, _, planes, bits, size, offset = struct.unpack_from("<BBBBHHII", ico, 6 + 16 * i)
        res += resource(RT_ICON, i + 1, ico[offset:offset + size])
        group += struct.pack("<BBBBHHIH", w, h, colors, 0, planes, bits, size, i + 1)
    res += resource(RT_GROUP_ICON, 1, group, flags=0x1030)
    open(os.path.join(here, "lumen.res"), "wb").write(res)


if __name__ == "__main__":
    main()
