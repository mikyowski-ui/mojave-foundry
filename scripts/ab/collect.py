#!/usr/bin/env python3
"""Collects the A/B runs into <root>/summary.md.

    python3 -I scripts/ab/collect.py --root out/readability-ab \
        --engine PATH --f2 PATH [--variant-dir NAME=DIR ...]

For each variant directory found under --root (in variants.conf order):
  * the FID budget: tiles, walls and scenery used. From a `## FID budget`
    table of map-report.md when it has one (any table whose header names
    tiles, walls and scenery; a cell is `N`, `N / LIMIT` or `N of LIMIT`);
    otherwise from the report as it is today: the "Art budget, per map and
    elevation" table (its `used walls / scenery / items` and `tiles`
    columns) and the "Art lists" table (entries used of the list's cap);
  * the build timings: the wall time the script measured (ab-done) and the
    stages of timings.txt;
  * the engine checks: engine-check/summary.txt (loads with exit 0 and no
    script refusals, each autoplay run's result line);
  * the sheet frames that failed, and the ready-to-paste scripts/play.sh
    command.
It never reads a file that is not there: a missing part is written as such.
"""
import argparse
import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import abconf  # noqa: E402

LIMIT = 4096
NUMBER = re.compile(r"(\d[\d,]*)")

# Flags recommended for a play session; see the summary's own notes.
PLAY_FLAGS = "--viewport unlocked_widescreen --zoom 0.75 --camera-follow smooth"


def read(path):
    try:
        with open(path, encoding="utf-8", errors="replace") as handle:
            return handle.read()
    except OSError:
        return None


def num(text):
    match = NUMBER.search(text or "")
    return int(match.group(1).replace(",", "")) if match else None


def used_of(cell):
    """(used, limit or None) of `N`, `N / L` or `N of L`."""
    parts = re.split(r"\s*(?:/|\bof\b)\s*", cell.strip(), maxsplit=1)
    used = num(parts[0])
    limit = num(parts[1]) if len(parts) > 1 else None
    return used, limit


def tables(section):
    """Markdown tables of a section: lists of rows (lists of cells)."""
    found, current = [], []
    for line in section.splitlines():
        if line.startswith("|"):
            cells = [c.strip() for c in line.strip().strip("|").split("|")]
            if all(re.fullmatch(r":?-{2,}:?", c) for c in cells):
                continue
            current.append(cells)
        elif current:
            found.append(current)
            current = []
    if current:
        found.append(current)
    return found


def section_of(report, heading):
    """The text under `heading` (a `## ` or `### ` line) up to the next
    heading of the same or a higher level."""
    lines = report.splitlines()
    for i, line in enumerate(lines):
        if line.strip().lower().startswith(heading.lower()):
            level = len(line) - len(line.lstrip("#"))
            out = []
            for later in lines[i + 1:]:
                if later.startswith("#") and len(later) - len(later.lstrip("#")) <= level:
                    break
                out.append(later)
            return "\n".join(out)
    return None


def budget_rows(report):
    """Rows of the FID budget: (label, tiles, walls, scenery, source); each
    count is (used, limit or None). Plus a note on what it was read from."""
    section = section_of(report, "## FID budget")
    if section:
        rows = []
        for table in tables(section):
            header = [h.lower() for h in table[0]]

            def col(word):
                return next((i for i, h in enumerate(header) if word in h), None)

            t, w, s = col("tile"), col("wall"), col("scener")
            if t is None or w is None or s is None:
                continue
            for cells in table[1:]:
                if len(cells) <= max(t, w, s):
                    continue
                rows.append((cells[0], used_of(cells[t]), used_of(cells[w]), used_of(cells[s])))
        if rows:
            return rows, "the report's `## FID budget` table"
    section = section_of(report, "### Art budget, per map and elevation")
    rows = []
    if section:
        for table in tables(section):
            header = [h.lower() for h in table[0]]
            used = next((i for i, h in enumerate(header) if h.startswith("used walls")), None)
            tiles = next((i for i, h in enumerate(header) if h == "tiles"), None)
            share = next((i for i, h in enumerate(header) if h.startswith("budget walls")), None)
            tile_share = next((i for i, h in enumerate(header) if h == "tile budget"), None)
            if used is None or tiles is None:
                continue
            for cells in table[1:]:
                if len(cells) <= max(used, tiles):
                    continue
                label = re.sub(r"\s*\(.*$", "", cells[0])
                triple = [num(x) for x in cells[used].split("/")]
                if len(triple) < 3 or None in triple[:3]:
                    continue
                budget = [num(x) for x in cells[share].split("/")] if share is not None and cells[share] else [None] * 3
                is_total = label.strip().lower() == "total"
                rows.append(
                    (
                        label,
                        (num(cells[tiles]), LIMIT if is_total else num(cells[tile_share]) if tile_share is not None else None),
                        (triple[0], LIMIT if is_total else budget[0]),
                        (triple[1], budget[1] if budget[1] is not None else None),
                    )
                )
    note = "the report's 'Art budget, per map and elevation' table (no `## FID budget` section yet; the limit shown for a map is its share of the layer's budget, for the total row 4,096)"
    return rows, note if rows else "no budget table found in map-report.md"


def list_rows(report):
    """Art lists: (name, used, cap) for tiles, walls and scenery."""
    section = section_of(report, "## Art lists")
    out = []
    if not section:
        return out
    for table in tables(section):
        header = [h.lower() for h in table[0]]
        if "list" not in header or "free after" not in header:
            continue
        col = {h: i for i, h in enumerate(header)}
        for cells in table[1:]:
            name = cells[col["list"]]
            if name not in ("tiles", "walls", "scenery"):
                continue
            replaced = cells[col["replaced"]].strip().lower() == "yes"
            base = num(cells[col["base entries"]]) or 0
            before = num(cells[col["free before"]]) or 0
            after = num(cells[col["free after"]]) or 0
            cap = before + (0 if replaced else base)
            out.append((name, cap - after, cap))
    return out


def timings_of(text):
    rows = []
    for line in (text or "").splitlines():
        match = re.match(r"^(\s*)(\d+(?:\.\d+)?)s\s+(.*\S)\s*$", line)
        if match and len(match.group(1)) <= 4:
            rows.append((match.group(3), float(match.group(2))))
    return rows


def checks_of(text):
    loads = []
    runs = []
    lines = (text or "").splitlines()
    for i, line in enumerate(lines):
        match = re.match(r"^load (\S+): exit (\d+); (.*?); script refusals (\d+)", line)
        if match:
            loads.append((match.group(1), int(match.group(2)), match.group(3), int(match.group(4))))
            continue
        match = re.match(r"^autoplay (\S+) \(exit (\d+)\):", line)
        if match:
            info = {"name": match.group(1), "exit": int(match.group(2)), "result": "?", "ticks": "", "bot": ""}
            for later in lines[i + 1:i + 14]:
                if not later.startswith("  "):
                    break
                body = later.strip()
                if body.startswith("result:"):
                    info["result"] = body[len("result:"):].strip()
                elif body.startswith("ticks"):
                    info["ticks"] = body
                elif body.startswith("bot:"):
                    info["bot"] = body
            runs.append(info)
    return loads, runs


def start_map(folder):
    text = read(os.path.join(folder, "mods", "mojave", "vault-mod.toml")) or ""
    match = re.search(r'^file = "([^"]+)\.map"', text, re.M)
    return match.group(1) if match else None


def fmt(pair):
    used, limit = pair
    if used is None:
        return "-"
    return "%s / %s" % (format(used, ","), format(limit, ",")) if limit else format(used, ",")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--conf", default=abconf.DEFAULT_CONF)
    parser.add_argument("--root", required=True)
    parser.add_argument("--engine", default="ENGINE")
    parser.add_argument("--f2", default="F2_DIR")
    parser.add_argument("--variant-dir", action="append", default=[], metavar="NAME=DIR")
    parser.add_argument("--output")
    args = parser.parse_args()

    conf = abconf.parse(args.conf)
    dirs = dict(item.split("=", 1) for item in args.variant_dir)
    found = []
    for variant in conf.variants:
        folder = dirs.get(variant["name"], os.path.join(args.root, variant["name"]))
        if os.path.isdir(folder):
            found.append((variant, folder))
    out = ["# Readability A/B: summary", ""]
    out.append("Variants found: %s." % (", ".join(v["name"] for v, _ in found) or "none"))
    out.append("")
    out.append("## Variants")
    out.append("")
    out.append("| variant | group | `--readability` pairs |")
    out.append("|---|---|---|")
    for variant, _ in found:
        out.append("| %s | %s | %s |" % (variant["name"], variant["group"], " ".join(variant["readability"]) or "(none: today's settings)"))
    out.append("")

    out.append("## Build timings")
    out.append("")
    out.append("Wall time is what `scripts/readability-ab.sh` measured around `mojave-convert all`; the stages are the converter's own `timings.txt`.")
    out.append("")
    stage_names = []
    per = {}
    for variant, folder in found:
        marker = read(os.path.join(folder, "ab-done")) or ""
        wall = re.search(r"^wall_s=(\S+)", marker, re.M)
        stages = timings_of(read(os.path.join(folder, "timings.txt")))
        per[variant["name"]] = (wall.group(1) if wall else "-", dict(stages))
        for name, _ in stages:
            if name not in stage_names and name != "total":
                stage_names.append(name)
    out.append("| variant | wall (s) | converter total (s) | " + " | ".join(stage_names) + " |")
    out.append("|---|---:|---:|" + "---:|" * len(stage_names))
    for variant, _ in found:
        wall, stages = per[variant["name"]]
        out.append(
            "| %s | %s | %s | %s |"
            % (variant["name"], wall, stages.get("total", "-"), " | ".join(str(stages.get(n, "-")) for n in stage_names))
        )
    out.append("")

    out.append("## FID budget")
    out.append("")
    out.append("Entries used against the format's 4,096 (tiles: the terrain library cap).")
    out.append("")
    for variant, folder in found:
        report = read(os.path.join(folder, "map-report.md"))
        out.append("### %s" % variant["name"])
        out.append("")
        if report is None:
            out.append("No map-report.md.")
            out.append("")
            continue
        rows, note = budget_rows(report)
        out.append("Read from %s." % note)
        out.append("")
        if rows:
            out.append("| map, elevation | tiles | walls | scenery |")
            out.append("|---|---:|---:|---:|")
            for label, tiles, walls, scenery in rows:
                out.append("| %s | %s | %s | %s |" % (label, fmt(tiles), fmt(walls), fmt(scenery)))
            out.append("")
        lists = list_rows(report)
        if lists:
            out.append("List totals (entries in the shipped lists, of the list's cap): " + "; ".join("%s %s / %s" % (n, format(u, ","), format(c, ",")) for n, u, c in lists) + ".")
            out.append("")

    out.append("## Engine checks")
    out.append("")
    out.append("| variant | maps loaded clean (exit 0, 0 script refusals) | bot run results |")
    out.append("|---|---|---|")
    details = []
    for variant, folder in found:
        text = read(os.path.join(folder, "engine-check", "summary.txt"))
        if text is None:
            out.append("| %s | not run | not run |" % variant["name"])
            continue
        loads, runs = checks_of(text)
        good = [l for l in loads if l[1] == 0 and l[3] == 0]
        bad = [l[0] for l in loads if not (l[1] == 0 and l[3] == 0)]
        load_text = "%d of %d" % (len(good), len(loads))
        if bad:
            load_text += " (not clean: %s)" % ", ".join(bad)
        run_text = "; ".join("%s: exit %d, result %s" % (r["name"], r["exit"], r["result"]) for r in runs) or "no run found"
        out.append("| %s | %s | %s |" % (variant["name"], load_text, run_text))
        for r in runs:
            details.append("- %s, %s: %s; %s" % (variant["name"], r["name"], r["ticks"] or "no tick line", r["bot"] or "no bot line"))
    out.append("")
    if details:
        out.extend(details)
        out.append("")

    out.append("## Sheet frames")
    out.append("")
    for variant, folder in found:
        text = read(os.path.join(folder, "ab-shots", "shots.json"))
        if text is None:
            out.append("- %s: no frames taken." % variant["name"])
            continue
        shots = json.loads(text)
        bad = ["%s (%s)" % (s["id"], s["error"]) for s in shots if not s["file"]]
        out.append("- %s: %d of %d frames%s" % (variant["name"], len(shots) - len(bad), len(shots), "; missing: " + "; ".join(bad) if bad else "."))
    out.append("")

    out.append("## Play commands")
    out.append("")
    out.append("`scripts/play.sh` takes the engine, the Fallout 2 directory, the output directory, an optional map and then passes everything else to the engine (it adds `--game --sfall --preset enhanced` itself). Recommended extra flags: `%s` (widescreen across the window, zoom 0.75, smooth camera follow). `--preset`, `--viewport`, `--zoom` and `--camera-follow` can each be changed in play: Alt+V toggles the viewport, Alt+C the camera follow, F11 fullscreen." % PLAY_FLAGS)
    out.append("")
    out.append("```sh")
    for variant, folder in found:
        rel = os.path.relpath(folder, abconf.ROOT) if folder.startswith(abconf.ROOT) else folder
        start = start_map(folder) or ""
        out.append('scripts/play.sh "%s" "%s" "%s" %s %s' % (args.engine, args.f2, rel, start, PLAY_FLAGS))
    out.append("```")
    out.append("")
    path = args.output or os.path.join(args.root, "summary.md")
    with open(path, "w", encoding="utf-8") as handle:
        handle.write("\n".join(out))
    print("collect: %s" % path)
    return 0


if __name__ == "__main__":
    sys.exit(main())
