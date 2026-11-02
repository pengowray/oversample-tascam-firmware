#!/usr/bin/env python3
"""Unpack and repack Tascam Portacapture X8 firmware update files.

An update file (for example `#PortaX8_0217.203`) is a 32-byte header followed
by the flash image, with every image byte replaced through a fixed 256-entry
substitution table. There is no key and no per-byte state.

Header layout (big-endian):
    0   8  magic "DR-TURBO" (Tascam's internal project name)
    8   2  version, e.g. 203 for V2.03
    10  2  build, e.g. 217
    12  2  year
    14  1  month
    15  1  day
    16  4  sum32: sum of all image bytes except the last 4, kept to 32 bits
    20  4  total file length (header + image)
    24  8  zero

The last 4 bytes of the image repeat sum32 (big-endian).

The table was worked out by the DR-05 homebrew project
(https://github.com/dr05-homebrew/dr05-homebrew, fwupdate/lut_decode.bin) and
found to apply to the X8 in the qubero hex editor
(crates/core/src/codec/tascam.rs).

Standard library only. Usage:
    x8fw.py info   UPDATE_FILE
    x8fw.py unpack UPDATE_FILE IMAGE.bin
    x8fw.py pack   IMAGE.bin UPDATE_FILE --version 203 --build 217 --date 2025-08-19
    x8fw.py selftest UPDATE_FILE...
"""

import argparse
import struct
import sys

MAGIC = b"DR-TURBO"
HEADER_LEN = 32
HEADER = struct.Struct(">8sHHHBBII8s")

# UNSCRAMBLE[b] is the image byte that the file byte b stands for.
UNSCRAMBLE = bytes([
    0x00, 0xcc, 0xc1, 0xc2, 0xc3, 0xcd, 0xce, 0xcf, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xcb,
    0x20, 0x2a, 0x2b, 0x2c, 0x2d, 0x24, 0x2e, 0x2f, 0x21, 0x22, 0x23, 0x25, 0x26, 0x27, 0x28, 0x29,
    0xd0, 0xd8, 0xd9, 0xda, 0xdb, 0xdd, 0xde, 0xdf, 0xd1, 0xdc, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7,
    0x40, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x4b, 0x4c, 0x4d, 0x4e, 0x4f, 0x41, 0x42, 0x43, 0x44,
    0xf0, 0xfb, 0xfc, 0xfd, 0xf2, 0xf3, 0xfe, 0x14, 0xf1, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
    0x70, 0x7b, 0x7c, 0x7d, 0x7e, 0x71, 0x72, 0x7f, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a,
    0xe0, 0xeb, 0xec, 0xed, 0xee, 0xe2, 0xe3, 0xef, 0xe1, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea,
    0xc0, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09,
    0x60, 0x66, 0x67, 0x68, 0x69, 0x6d, 0x6e, 0x6f, 0x6a, 0x6b, 0x6c, 0x61, 0x62, 0x63, 0x64, 0x65,
    0x80, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x81, 0x89, 0x8a, 0x8b, 0x8c, 0x8d, 0x8e, 0x8f,
    0x30, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x3e, 0x3f, 0x31, 0x32, 0x33, 0x3b, 0x3c, 0x3d, 0x34, 0x35,
    0x50, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x5b, 0x5f, 0x51, 0x5c, 0x5d, 0x5e, 0x52, 0x53, 0x54, 0x55,
    0xa0, 0xac, 0xad, 0xae, 0xa2, 0xa3, 0xa4, 0xaf, 0xa1, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xab,
    0x90, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9f, 0x9a, 0x9b, 0x9c, 0x9d, 0x9e, 0x91, 0x92, 0x93,
    0xb0, 0xbc, 0xbd, 0xbe, 0xb1, 0xb2, 0xb3, 0xbf, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xbb,
    0x10, 0x15, 0x16, 0x17, 0x1c, 0x1d, 0x1e, 0x1f, 0x11, 0x12, 0x13, 0x18, 0x19, 0x1a, 0x1b, 0xff,
])
SCRAMBLE = bytes(UNSCRAMBLE.index(i) for i in range(256))


def sum32(data):
    return sum(data) & 0xFFFFFFFF


def parse(update):
    if len(update) < HEADER_LEN + 4:
        raise ValueError("file too short to be an update file")
    magic, version, build, year, month, day, hdr_sum, total, pad = HEADER.unpack_from(update)
    if magic != MAGIC:
        raise ValueError(f"magic is {magic!r}, expected {MAGIC!r}")
    image = update[HEADER_LEN:].translate(UNSCRAMBLE)
    info = {
        "version": version,
        "build": build,
        "date": f"{year:04d}-{month:02d}-{day:02d}",
        "header_sum32": hdr_sum,
        "trailer_sum32": struct.unpack(">I", image[-4:])[0],
        "computed_sum32": sum32(image[:-4]),
        "header_length": total,
        "file_length": len(update),
        "image_length": len(image),
        "padding_zero": pad == bytes(8),
        "starts_with_FCFB": image[:4] == b"FCFB",
    }
    return info, image


def build_update(image, version, build, date):
    year, month, day = (int(x) for x in date.split("-"))
    image = bytearray(image)
    s = sum32(image[:-4])
    image[-4:] = struct.pack(">I", s)
    total = HEADER_LEN + len(image)
    header = HEADER.pack(MAGIC, version, build, year, month, day, s, total, bytes(8))
    return header + bytes(image).translate(SCRAMBLE)


def check(info):
    problems = []
    if info["header_sum32"] != info["computed_sum32"]:
        problems.append("header sum32 does not match the image")
    if info["trailer_sum32"] != info["computed_sum32"]:
        problems.append("image trailer sum32 does not match the image")
    if info["header_length"] != info["file_length"]:
        problems.append("header length field does not match the file length")
    if not info["padding_zero"]:
        problems.append("header bytes 24-31 are not zero")
    if not info["starts_with_FCFB"]:
        problems.append("image does not start with the FlexSPI config block tag FCFB")
    return problems


def print_info(info):
    print(f"version        V{info['version'] // 100}.{info['version'] % 100:02d} ({info['version']})")
    print(f"build          {info['build']}")
    print(f"date           {info['date']}")
    print(f"file length    {info['file_length']:,} bytes (header says {info['header_length']:,})")
    print(f"image length   {info['image_length']:,} bytes")
    print(f"sum32          header 0x{info['header_sum32']:08x}, trailer 0x{info['trailer_sum32']:08x}, "
          f"computed 0x{info['computed_sum32']:08x}")
    problems = check(info)
    print("checks         " + ("all pass" if not problems else "; ".join(problems)))
    return problems


def main(argv):
    p = argparse.ArgumentParser(description="Unpack and repack Portacapture X8 firmware update files.")
    sub = p.add_subparsers(dest="cmd", required=True)
    a = sub.add_parser("info", help="show the header and check the sums")
    a.add_argument("update")
    a = sub.add_parser("unpack", help="write the descrambled flash image")
    a.add_argument("update")
    a.add_argument("image")
    a = sub.add_parser("pack", help="build an update file from a flash image, fixing both sums")
    a.add_argument("image")
    a.add_argument("update")
    a.add_argument("--version", type=int, required=True, help="e.g. 203 for V2.03")
    a.add_argument("--build", type=int, required=True)
    a.add_argument("--date", required=True, help="YYYY-MM-DD")
    a = sub.add_parser("selftest", help="unpack and repack each file and compare bytes")
    a.add_argument("updates", nargs="+")
    args = p.parse_args(argv)

    if args.cmd == "info":
        info, _ = parse(open(args.update, "rb").read())
        return 1 if print_info(info) else 0

    if args.cmd == "unpack":
        info, image = parse(open(args.update, "rb").read())
        problems = print_info(info)
        open(args.image, "wb").write(image)
        print(f"wrote {args.image}")
        return 1 if problems else 0

    if args.cmd == "pack":
        image = open(args.image, "rb").read()
        out = build_update(image, args.version, args.build, args.date)
        open(args.update, "wb").write(out)
        info, _ = parse(out)
        print_info(info)
        print(f"wrote {args.update}")
        return 0

    if args.cmd == "selftest":
        failed = 0
        for path in args.updates:
            original = open(path, "rb").read()
            info, image = parse(original)
            rebuilt = build_update(image, info["version"], info["build"], info["date"])
            same = rebuilt == original
            problems = check(info)
            status = "ok" if same and not problems else "FAILED"
            print(f"{status}: {path} (repack identical: {same}; checks: {problems or 'all pass'})")
            failed += status != "ok"
        return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
