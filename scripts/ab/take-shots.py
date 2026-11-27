#!/usr/bin/env python3
"""Takes one variant's contact-sheet frames: the same world spot in every variant.

    python3 -I scripts/ab/take-shots.py --variant NAME --dir OUT_DIR \
        --converter mojave-convert --engine vault-platform --f2 FALLOUT2_DIR

The viewpoints of the variant's group (scripts/ab/variants.conf) are resolved
by reference in OUT_DIR/map-placements.json (abconf.resolve). Frames go to
OUT_DIR/ab-shots/<viewpoint id>.png, 640x380 classic frames, and
OUT_DIR/ab-shots/shots.json records what was taken.

Elevation 0: one `mojave-convert shots --classic --at HEX ...` per map (the
entry is always photographed as well). The frame is matched to its viewpoint
by the hex asked for, from the run's index.json.

Elevation 1 and 2: `shots` always starts on elevation 0 and routes there, so
the helper uses `mojave-convert walkthrough --scene`: through the elevation-0
stairs nearest the target, then to the target on the upper elevation. The
scene asks for a single frame at a tick past the end, which the tool answers
with the last frame of the walk (one engine run, not hundreds).
"""
import argparse
import json
import os
import shutil
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import abconf  # noqa: E402


def run(command, log_path):
    with open(log_path, "w", encoding="utf-8") as log:
        log.write("$ " + " ".join(command) + "\n")
        log.flush()
        return subprocess.call(command, stdout=log, stderr=subprocess.STDOUT)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--conf", default=abconf.DEFAULT_CONF)
    parser.add_argument("--variant", required=True)
    parser.add_argument("--dir", required=True)
    parser.add_argument("--converter", required=True)
    parser.add_argument("--engine", required=True)
    parser.add_argument("--f2", required=True)
    parser.add_argument("--jobs", default="2")
    parser.add_argument("--timeout", default="600")
    args = parser.parse_args()

    conf = abconf.parse(args.conf)
    rows = abconf.resolve(conf, args.variant, args.dir, args.converter)
    shots_dir = os.path.join(args.dir, "ab-shots")
    raw = os.path.join(shots_dir, "raw")
    shutil.rmtree(shots_dir, ignore_errors=True)
    os.makedirs(raw)
    common = ["--engine", args.engine, "--f2", args.f2, "--out", args.dir]
    status = {}

    def finish(row, file, error=None):
        entry = {
            "id": row["id"],
            "label": row["label"],
            "map": row["map"],
            "elev": row["elev"],
            "hex": row["hex"],
            "exit": row["exit"],
            "file": file,
            "error": error or row["error"],
        }
        status[row["id"]] = entry

    # Elevation 0, one `shots` run per map.
    maps = []
    for row in rows:
        if row["elev"] == 0 and not row["error"] and row["map"] not in maps:
            maps.append(row["map"])
    for map_file in maps:
        mine = [r for r in rows if r["map"] == map_file and r["elev"] == 0 and not r["error"]]
        dest = os.path.join(raw, map_file[:-4])
        command = [args.converter, "shots"] + common + [
            "--map", map_file, "--classic", "--jobs", args.jobs,
            "--timeout", args.timeout, "--dest", dest,
        ]
        for r in mine:
            if not r["entry"]:
                command += ["--at", str(r["hex"])]
        code = run(command, os.path.join(raw, map_file[:-4] + ".log"))
        index = None
        try:
            with open(os.path.join(dest, "index.json"), encoding="utf-8") as handle:
                index = json.load(handle)
        except (OSError, ValueError):
            pass
        for r in mine:
            if index is None:
                finish(r, None, "shots failed (exit %d); see ab-shots/raw/%s.log" % (code, map_file[:-4]))
                continue
            wanted = index["entry_hex"] if r["entry"] else r["hex"]
            # The first shot of that view of the hex asked for (viewpoint 0
            # is the entry); `requested_hex` is what --at was given.
            hit = next(
                (s for s in index["shots"]
                 if s["requested_hex"] == wanted and s["view"] == "classic"
                 and s["file"] and not s["error"]),
                None,
            )
            if not hit:
                finish(r, None, "no frame for hex %s in the shots index" % wanted)
                continue
            target = os.path.join(shots_dir, r["id"] + ".png")
            shutil.copyfile(os.path.join(dest, hit["file"]), target)
            r = dict(r, hex=hit["hex"], note="walked to hex %d (asked %d)" % (hit["hex"], wanted))
            finish(r, r["id"] + ".png")

    # Elevation 1 and 2, one walk-through each.
    for r in rows:
        if r["elev"] == 0 or r["error"]:
            continue
        name = r["id"]
        scene = {
            "name": name,
            "map": r["map"],
            "view": "classic",
            "through": [r["exit"], r["hex"]],
            "first": 1000000000,
            "frames": 1,
        }
        scene_path = os.path.join(raw, name + ".scene.json")
        with open(scene_path, "w", encoding="utf-8") as handle:
            json.dump(scene, handle)
        dest = os.path.join(raw, name)
        command = [args.converter, "walkthrough"] + common + [
            "--scene", scene_path, "--classic", "--no-gif", "--jobs", args.jobs,
            "--timeout", args.timeout, "--dest", dest,
        ]
        code = run(command, os.path.join(raw, name + ".log"))
        frames_dir = os.path.join(dest, name, "classic")
        frames = sorted(f for f in os.listdir(frames_dir) if f.endswith(".png")) if os.path.isdir(frames_dir) else []
        if code != 0 or not frames:
            finish(r, None, "walkthrough failed (exit %d); see ab-shots/raw/%s.log" % (code, name))
            continue
        shutil.copyfile(os.path.join(frames_dir, frames[-1]), os.path.join(shots_dir, name + ".png"))
        finish(r, name + ".png")

    for r in rows:
        if r["id"] not in status:
            finish(r, None)
    with open(os.path.join(shots_dir, "shots.json"), "w", encoding="utf-8") as handle:
        json.dump([status[r["id"]] for r in rows], handle, indent=1)
        handle.write("\n")
    failed = [s for s in status.values() if not s["file"]]
    for s in failed:
        print("take-shots: %s: %s" % (s["id"], s["error"]), file=sys.stderr)
    print("take-shots: %d of %d frames in %s" % (len(status) - len(failed), len(status), shots_dir))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
