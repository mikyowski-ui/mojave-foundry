#!/usr/bin/env python3
"""Builds a group's side-by-side contact sheet with `mojave-convert contact`.

    python3 -I scripts/ab/make-sheet.py --group megaton --root out/readability-ab \
        --converter mojave-convert --output out/readability-ab/megaton-ab-sheet.png

One row per viewpoint of the group (variants.conf order), one column per
variant that has frames (<root>/<variant>/ab-shots/<id>.png), each picture
labelled `<variant> | <viewpoint label>`. `contact` has no row or column
headers, so the labels carry both. A frame that could not be taken is
replaced by a plain grey 640x380 picture labelled `<variant> | <label> (no
frame)`.
--root may also be given with --variant-dir NAME=DIR for out-of-tree runs.
"""
import argparse
import os
import struct
import subprocess
import sys
import zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import abconf  # noqa: E402


def grey_png(path, width=640, height=380):
    row = b"\x00" + bytes([60, 60, 60]) * width
    raw = row * height

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw))
        + chunk(b"IEND", b"")
    )
    with open(path, "wb") as handle:
        handle.write(png)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--conf", default=abconf.DEFAULT_CONF)
    parser.add_argument("--group", required=True)
    parser.add_argument("--root", required=True)
    parser.add_argument("--converter", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--title")
    parser.add_argument("--variant-dir", action="append", default=[], metavar="NAME=DIR")
    args = parser.parse_args()

    conf = abconf.parse(args.conf)
    dirs = dict(item.split("=", 1) for item in args.variant_dir)
    columns = []
    for variant in conf.of_group(args.group):
        folder = dirs.get(variant["name"], os.path.join(args.root, variant["name"]))
        if os.path.isdir(os.path.join(folder, "ab-shots")):
            columns.append((variant["name"], folder))
    if not columns:
        print("make-sheet: no variant of group %s has frames" % args.group, file=sys.stderr)
        return 1
    rows = conf.viewpoints_of(args.group)
    scratch = os.path.join(os.path.dirname(os.path.abspath(args.output)), ".sheet-" + args.group)
    os.makedirs(scratch, exist_ok=True)
    grey = os.path.join(scratch, "grey.png")
    grey_png(grey)
    pictures = []
    for view in rows:
        for name, folder in columns:
            path = os.path.join(folder, "ab-shots", view["id"] + ".png")
            label = "%s | %s" % (name, view["label"])
            if not os.path.isfile(path):
                path, label = grey, label + " (no frame)"
            pictures.append("%s=%s" % (label.replace("=", ":"), path))
    title = args.title or "Readability A/B: %s (columns: %s)" % (
        args.group,
        ", ".join(n for n, _ in columns),
    )
    command = [
        args.converter, "contact", "--out", args.output, "--title", title,
        "--columns", str(len(columns)), "--cell", "640x380",
    ] + pictures
    code = subprocess.call(command)
    if code == 0:
        print("make-sheet: %s (%d rows x %d columns)" % (args.output, len(rows), len(columns)))
    return code


if __name__ == "__main__":
    sys.exit(main())
