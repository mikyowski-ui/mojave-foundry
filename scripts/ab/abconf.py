"""Shared code of the readability A/B helpers (Python 3 standard library only).

Parses scripts/ab/variants.conf and resolves viewpoints. The helpers are run
with `python3 -I`, which keeps the script directory off the import path, so
each of them adds this directory itself before importing this module.
"""

import json
import os
import re
import shlex
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
DEFAULT_CONF = os.path.join(HERE, "variants.conf")


class ConfError(Exception):
    pass


class Conf:
    """The parsed definition file."""

    def __init__(self):
        self.variables = {}
        self.groups = {}
        self.variants = []  # dicts: name, group, readability (list of "k=v")
        self.viewpoints = []  # dicts: group, id, map, elev, target, label

    def variant(self, name):
        for v in self.variants:
            if v["name"] == name:
                return v
        raise ConfError("no variant named %s" % name)

    def of_group(self, group):
        return [v for v in self.variants if v["group"] == group]

    def viewpoints_of(self, group):
        return [p for p in self.viewpoints if p["group"] == group]

    def missing_variables(self, variant):
        """Variables a variant's pairs name that are empty."""
        return [n for n in variant.get("needs", []) if not self.variables.get(n)]


def parse(path=DEFAULT_CONF):
    conf = Conf()
    with open(path, encoding="utf-8") as handle:
        lines = handle.read().splitlines()
    for number, raw in enumerate(lines, 1):
        text = raw.strip()
        if not text or text.startswith("#"):
            continue

        def fail(message):
            raise ConfError("%s:%d: %s" % (path, number, message))

        match = re.match(r"^([A-Z][A-Z0-9_]*)=(.*)$", text)
        if match:
            value = match.group(2).split("#", 1)[0].strip()
            conf.variables[match.group(1)] = value
            continue
        needs = re.findall(r"\$\{([A-Z][A-Z0-9_]*)\}", text)
        for name in needs:
            if name not in conf.variables:
                fail("unknown variable ${%s}" % name)
            text = text.replace("${%s}" % name, conf.variables[name])
        try:
            words = shlex.split(text, comments=True)
        except ValueError as error:
            fail(str(error))
        keyword, rest = words[0], words[1:]
        if keyword == "group":
            if not rest:
                fail("group needs a name")
            fields = dict_of(rest[1:], fail)
            for key in ("layer", "base"):
                if key not in fields:
                    fail("group %s needs %s=" % (rest[0], key))
            conf.groups[rest[0]] = fields
        elif keyword == "variant":
            if len(rest) < 2:
                fail("variant NAME group=G ...")
            name, others = rest[0], rest[1:]
            group, pairs = None, []
            for word in others:
                if word.startswith("group="):
                    group = word[len("group="):]
                elif "=" in word:
                    pairs.append(word)
                else:
                    fail("not KEY=VALUE: %s" % word)
            if group not in conf.groups:
                fail("variant %s: unknown group %s" % (name, group))
            conf.variants.append(
                {
                    "name": name,
                    "group": group,
                    "readability": pairs,
                    "needs": needs,
                }
            )
        elif keyword == "viewpoint":
            if len(rest) < 2:
                fail("viewpoint GROUP ID map=.. elev=.. target=.. label=..")
            fields = dict_of(rest[2:], fail)
            for key in ("map", "elev", "target"):
                if key not in fields:
                    fail("viewpoint %s needs %s=" % (rest[1], key))
            conf.viewpoints.append(
                {
                    "group": rest[0],
                    "id": rest[1],
                    "map": fields["map"],
                    "elev": int(fields["elev"]),
                    "target": fields["target"],
                    "label": fields.get("label", rest[1]),
                }
            )
        else:
            fail("unknown definition %r" % keyword)
    return conf


def dict_of(words, fail):
    out = {}
    for word in words:
        if "=" not in word:
            fail("not KEY=VALUE: %s" % word)
        key, value = word.split("=", 1)
        out[key] = value
    return out


# --------------------------------------------------------------- placements


def load_placements(out_dir):
    path = os.path.join(out_dir, "map-placements.json")
    with open(path, encoding="utf-8") as handle:
        data = json.load(handle)
    if data.get("schema_version") != 2:
        raise ConfError(
            "%s: schema_version %r, expected 2" % (path, data.get("schema_version"))
        )
    return data


def find_reference(placements, stem, elevation, target):
    """The first placed reference named by `target` (alternatives split on
    `|`) on map `stem` at `elevation`: (hex, which alternative) or None.

    A reference placed as several objects keeps the hex of its first one.
    A copy of a person on a second map carries `@map` after its id; it is
    never taken for the original (the exact id must match).
    """
    placed = [
        p
        for p in placements.get("placed", [])
        if p.get("map") == stem and p.get("elevation") == elevation
    ]
    for alternative in target.split("|"):
        if alternative.startswith("editor:"):
            wanted = alternative[len("editor:"):]
            hits = [p for p in placed if p.get("editor_id") == wanted]
        else:
            hits = [p for p in placed if p.get("reference") == alternative]
        if hits:
            return hits[0]["hex"], alternative
    return None


# --------------------------------------------------------------- hex geometry


def cube(hex_number):
    """Cube coordinates of a hex number (an odd-q layout on a 200-wide grid).

    Which columns are shifted is a convention; the engine's own may be the
    other one, so distances are good to one hex, which is all that choosing
    the nearest of several stairs needs.
    """
    column, row = hex_number % 200, hex_number // 200
    x = column
    z = row - (column - (column & 1)) // 2
    return x, -x - z, z


def distance(a, b):
    ax, ay, az = cube(a)
    bx, by, bz = cube(b)
    return max(abs(ax - bx), abs(ay - by), abs(az - bz))


# --------------------------------------------------------------- exits

EXIT_LINE = re.compile(
    r"^exit (\d+) -> map (\d+) hex (\d+)(?: elevation (\d+))?:"
)


def exits_of(converter, out_dir, map_file):
    """The exit grids of elevation 0 of a map, from `probe walk`:
    (grid hex, destination map number, destination elevation, arrival hex).
    """
    layer = os.path.join(out_dir, "mods", "mojave")
    result = subprocess.run(
        [converter, "probe", "walk", layer, map_file, "--elevation", "0", "--hop", "400"],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        universal_newlines=True,
    )
    if result.returncode != 0:
        raise ConfError("probe walk %s failed: %s" % (map_file, result.stdout[-300:]))
    grids = []
    for line in result.stdout.splitlines():
        match = EXIT_LINE.match(line)
        if match:
            grids.append(
                (
                    int(match.group(1)),
                    int(match.group(2)),
                    int(match.group(4) or 0),
                    int(match.group(3)),
                )
            )
    return grids


def map_number(placements, stem):
    for entry in placements.get("maps", []):
        if entry.get("stem") == stem:
            return entry["number"]
    raise ConfError("map %s is not in map-placements.json" % stem)


def resolve(conf, variant_name, out_dir, converter):
    """Resolves the viewpoints of the variant's group in `out_dir`.

    Returns dicts: id, map, elev, label, hex (None for the entry), entry
    (True for `target=entry`), exit (the elevation-0 grid to cross first, for
    elevation 1 and 2), note, error.
    """
    variant = conf.variant(variant_name)
    placements = load_placements(out_dir)
    resolved = []
    for view in conf.viewpoints_of(variant["group"]):
        row = {
            "id": view["id"],
            "map": view["map"],
            "elev": view["elev"],
            "label": view["label"],
            "hex": None,
            "entry": False,
            "exit": None,
            "arrival": None,
            "note": "",
            "error": None,
        }
        stem = view["map"][:-4] if view["map"].endswith(".map") else view["map"]
        if view["target"] == "entry":
            row["entry"] = True
            resolved.append(row)
            continue
        found = find_reference(placements, stem, view["elev"], view["target"])
        elev = view["elev"]
        if not found:
            # A turn or a spread can move a deck to another elevation of the
            # same map: look for the reference there before giving up.
            for other in (1, 2, 0):
                if other == elev:
                    continue
                found = find_reference(placements, stem, other, view["target"])
                if found:
                    elev = other
                    row["elev"] = other
                    break
        if not found:
            row["error"] = "no reference of %s placed on %s elevation %d" % (
                view["target"],
                view["map"],
                view["elev"],
            )
            resolved.append(row)
            continue
        row["hex"], used = found
        row["note"] = used
        if elev > 0:
            number = map_number(placements, stem)
            options = [
                g
                for g in exits_of(converter, out_dir, view["map"])
                if g[1] == number and g[2] == elev
            ]
            if not options:
                row["error"] = "no stairs from elevation 0 lead to elevation %d" % elev
            else:
                best = min(options, key=lambda g: (distance(g[3], row["hex"]), g[0]))
                row["exit"], row["arrival"] = best[0], best[3]
        resolved.append(row)
    return resolved


def main(argv):
    """`variants`: one tab-separated line per variant for the shell script:
    name, group, layer, base, missing variables (or -), pairs (or -)."""
    if len(argv) != 2 or argv[1] != "variants":
        print("usage: abconf.py variants", file=sys.stderr)
        return 2
    try:
        conf = parse()
    except (ConfError, OSError) as error:
        print("variants.conf: %s" % error, file=sys.stderr)
        return 1
    for v in conf.variants:
        group = conf.groups[v["group"]]
        print(
            "\t".join(
                [
                    v["name"],
                    v["group"],
                    group["layer"],
                    group["base"],
                    ",".join(conf.missing_variables(v)) or "-",
                    " ".join(v["readability"]) or "-",
                ]
            )
        )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
