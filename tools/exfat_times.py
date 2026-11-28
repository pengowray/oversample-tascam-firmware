#!/usr/bin/env python3
"""Print the raw time fields of every file entry in a folder of an exFAT volume.

Usage: sudo python3 tools/exfat_times.py /dev/sdc1 [FOLDER]     (FOLDER defaults to SOUND)

Reads only. For each entry: the three timestamps as stored, the 10 ms bytes, and the three UTC
offset bytes (top bit = offset is valid, low 7 bits = signed count of 15-minute steps).
"""
import struct
import sys


def main():
    device = sys.argv[1]
    folder = sys.argv[2] if len(sys.argv) > 2 else "SOUND"
    with open(device, "rb") as f:
        boot = f.read(512)
        if boot[3:11] != b"EXFAT   ":
            sys.exit("not an exFAT volume")
        fat_offset, _fat_length, heap_offset, _clusters, root = struct.unpack_from("<IIIII", boot, 80)
        sector = 1 << boot[108]
        cluster = sector << boot[109]

        def chain(first, contiguous=False, size=None):
            data, c = b"", first
            while 2 <= c < 0xFFFFFFF7:
                f.seek((heap_offset + (c - 2) * (cluster // sector)) * sector)
                data += f.read(cluster)
                if size is not None and len(data) >= size:
                    break
                if contiguous:
                    c += 1
                else:
                    f.seek(fat_offset * sector + c * 4)
                    c = struct.unpack("<I", f.read(4))[0]
            return data

        def entries(data):
            i = 0
            while i + 32 <= len(data):
                e = data[i:i + 32]
                if e[0] == 0:
                    return
                if e[0] == 0x85:
                    count = e[1]
                    stream = data[i + 32:i + 64]
                    name = b"".join(data[i + 64 + 32 * k + 2:i + 96 + 32 * k] for k in range(count - 1))
                    name = name.decode("utf-16-le")[:stream[3]]
                    yield name, e, stream
                    i += 32 * (count + 1)
                else:
                    i += 32

        def show(name, e):
            print(name)
            print("  raw entry:", e.hex(" "))
            for label, at, ms, tz in (("created ", 8, 20, 22), ("modified", 12, 21, 23), ("accessed", 16, None, 24)):
                t = struct.unpack_from("<I", e, at)[0]
                stamp = "%04d-%02d-%02d %02d:%02d:%02d" % (
                    1980 + (t >> 25), (t >> 21) & 15, (t >> 16) & 31, (t >> 11) & 31, (t >> 5) & 63, (t & 31) * 2)
                off = e[tz]
                if off & 0x80:
                    steps = (off & 0x7F) - 128 if off & 0x40 else off & 0x7F
                    zone = "valid, UTC%+d:%02d" % (steps * 15 // 60, abs(steps * 15) % 60)
                else:
                    zone = "not valid (no offset given)"
                tenms = "" if ms is None else "  10ms=%d" % e[ms]
                print("  %s %s%s  offset byte 0x%02X: %s" % (label, stamp, tenms, off, zone))

        data = chain(root)
        for name, e, stream in entries(data):
            if name.upper() == folder.upper():
                show(name + "/", e)
                first = struct.unpack_from("<I", stream, 20)[0]
                size = struct.unpack_from("<Q", stream, 24)[0]
                for n, e2, _ in entries(chain(first, bool(stream[1] & 2), size)):
                    show(n, e2)
                return
        sys.exit("no folder named " + folder)


if __name__ == "__main__":
    main()
