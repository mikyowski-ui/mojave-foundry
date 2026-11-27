#!/usr/bin/env python3
"""Resolves a variant's viewpoints (named by reference) to hexes.

    python3 -I scripts/ab/resolve-viewpoints.py --variant NAME --dir OUT_DIR \
        --converter mojave-convert [--conf scripts/ab/variants.conf] [--json]

OUT_DIR is the variant's `--out` directory (it holds map-placements.json and
mods/mojave). The converter is used only for elevation 1 and 2 targets, to
list the stairs of elevation 0 (`probe walk`). Prints one line per viewpoint
(or JSON); the exit status is 1 when any viewpoint could not be resolved.
"""
import argparse
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import abconf  # noqa: E402


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--conf", default=abconf.DEFAULT_CONF)
    parser.add_argument("--variant", required=True)
    parser.add_argument("--dir", required=True)
    parser.add_argument("--converter", default="mojave-convert")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    try:
        conf = abconf.parse(args.conf)
        rows = abconf.resolve(conf, args.variant, args.dir, args.converter)
    except (abconf.ConfError, OSError) as error:
        print("resolve-viewpoints: %s" % error, file=sys.stderr)
        return 2
    if args.json:
        json.dump(rows, sys.stdout, indent=1)
        print()
    else:
        for r in rows:
            where = "entry" if r["entry"] else ("hex %s" % r["hex"])
            if r["exit"] is not None:
                where += " via stairs %d (arrive %d)" % (r["exit"], r["arrival"])
            print(
                "%-9s %-12s e%d %s%s"
                % (r["id"], r["map"], r["elev"], where, "  ERROR: " + r["error"] if r["error"] else "")
            )
    return 1 if any(r["error"] for r in rows) else 0


if __name__ == "__main__":
    sys.exit(main())
