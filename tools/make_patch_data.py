"""Make the data the patcher web page applies: the bytes a patched update file changes.

    python3 tools/make_patch_data.py "Tascam_X8_official/extracted/#PortaX8_0217.203" \\
        "work/patch24/#PortaX8_0248.203" site/patches/B248

Writes `manifest.json` (both files' names, sizes and SHA-256) and `changes.bin` to the output
folder. `changes.bin` holds only the runs of bytes that differ, each as offset (u32,
little-endian), length (u32) and the new bytes, then the bytes the patched file has past the end of
the original. Runs are not merged across unchanged bytes, so none of TASCAM's bytes are copied: the
update file's scrambling is a fixed substitution of each byte, so a changed byte holds only the
patch's own data. The page refuses any file whose SHA-256 differs from the original's, and checks
the result's SHA-256 before offering it.
"""
import argparse
import hashlib
import json
import os
import struct


def runs(old, new):
    """(offset, bytes) for each run of bytes that differ, and the bytes past the end of `old`."""
    out, i, n = [], 0, min(len(old), len(new))
    while i < n:
        if old[i] == new[i]:
            i += 1
            continue
        start = i
        while i < n and old[i] != new[i]:
            i += 1
        out.append((start, new[start:i]))
    if len(new) > len(old):
        out.append((len(old), new[len(old):]))
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("original")
    ap.add_argument("patched")
    ap.add_argument("out_dir")
    ap.add_argument("--build", type=int, help="the patch's build number, for the manifest")
    args = ap.parse_args()
    old = open(args.original, "rb").read()
    new = open(args.patched, "rb").read()
    changes = runs(old, new)
    os.makedirs(args.out_dir, exist_ok=True)
    with open(os.path.join(args.out_dir, "changes.bin"), "wb") as f:
        for offset, data in changes:
            f.write(struct.pack("<II", offset, len(data)))
            f.write(data)
    name = os.path.basename(args.patched)
    build = args.build or int(name.split("_")[-1].split(".")[0])
    manifest = {
        "build": build,
        "original": {"name": os.path.basename(args.original), "size": len(old), "sha256": hashlib.sha256(old).hexdigest()},
        "patched": {"name": name, "size": len(new), "sha256": hashlib.sha256(new).hexdigest()},
        "runs": len(changes),
        "changed_bytes": sum(len(d) for _, d in changes),
    }
    with open(os.path.join(args.out_dir, "manifest.json"), "w") as f:
        json.dump(manifest, f, indent=2)
        f.write("\n")
    print(json.dumps(manifest, indent=2))


if __name__ == "__main__":
    main()
