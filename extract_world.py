"""Write a world file (the blocks of a course) from a Flashback replay.

usage: python3 extract_world.py REPLAY.zip -o OUT.world.gz [--tick T]
                                [--bounds X0 Y0 Z0 X1 Y1 Z1]

The world is the client's at the end of tick T (default: the replay's end; see
world.py for what that replays). Without --bounds the file holds every column the
client had then; with them, every section that meets the box (block coordinates,
inclusive), and sections in it the replay never loaded are written as unknown. A
coverage report goes to stdout. The solver reads the file with `crates/world`.

World file format, version 1. This docstring is the spec; `crates/world` reads it.

A world file is gzip of:

    line 1   header: a JSON object on one line (UTF-8)
    line 2   content head: a JSON object on one line
    rest     sections, binary, little-endian

The header's `content_sha256` is the sha256 of everything after line 1's newline
(line 2 and the sections). So it names the blocks and the block table that gives
them their shapes, not where they came from; a course file refers to its world by it.

Header (provenance):
    format          "etas-world"
    version         1; a reader rejects other versions
    source          replay file name, or a free-form note for hand-made worlds
    tick            the replay tick whose end state this is, or null
    dimension       dimension key ("minecraft:eburrowedgrove"), or null
    dimension_type  its dimension type key, or null
    min_y_method    how min_y was found: "registry" (the dimension type's
                    REGISTRY_DATA entry), "vanilla dimension type" (a vanilla key the
                    server sent without data), "vote" (a guess from on-ground poses,
                    see world.py), or "given"
    bounds          [[x0, y0, z0], [x1, y1, z1]], inclusive, or null: everything received
    coverage        informative counts (updates applied, columns missing, ...)
    content_sha256  hex

Content head:
    min_y, height   the dimension's build height. A cell with y outside
                    [min_y, min_y + height) is minecraft:air wherever it is, as in
                    LevelChunk.getBlockState. height null: no such rule.
    block_table     {"name": "blocks-26.2.tsv.gz", "sha256": sha256 of the
                    decompressed table}. A reader whose table differs rejects the file.
                    why? a cell's boxes, offset and effect class come from the table.
    block_tags      {"minecraft:climbable": [block names], "minecraft:can_glide_through":
                    [...]}: the server's, at the tick (a tag it didn't send is []), or
                    null: the table's vanilla columns. A block is climbable if it is in
                    climbable, and ends a glide if also not in can_glide_through.
    palette         full state strings, as in the table's state column

Sections:
    u32 count
    count times: i32 sx, i32 sy, i32 sz (section coordinates, block >> 4), u8 kind,
    then by kind:
        0  unknown: inside the bounds but never received; no payload
        1  u16 palette index for all 4096 cells
        2  4096 u16 palette indices, cell (x, y, z) at (y & 15) << 8 | (z & 15) << 4 | (x & 15);
           0xFFFF is an unknown cell
    Sections are unique and sorted by (sx, sz, sy). A section not listed is unknown.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import struct
import sys
from array import array
from pathlib import Path

import flashback as fb
import world

FORMAT, VERSION = "etas-world", 1
UNKNOWN_CELL = 0xFFFF


def sections_of(w: world.World, bounds=None):
    """[(sx, sy, sz, section or None for unknown)], sorted by (sx, sz, sy)."""
    base = w.min_y >> 4
    out = []
    if bounds is None:
        for (cx, cz), secs in w.chunks.items():
            out.extend((cx, base + i, cz, s) for i, s in enumerate(secs))
    else:
        (x0, y0, z0), (x1, y1, z1) = bounds
        if w.height is not None:
            y0, y1 = max(y0, w.min_y), min(y1, w.min_y + w.height - 1)
        for cx in range(x0 >> 4, (x1 >> 4) + 1):
            for cz in range(z0 >> 4, (z1 >> 4) + 1):
                secs = w.chunks.get((cx, cz))
                for sy in range(y0 >> 4, (y1 >> 4) + 1):
                    out.append((cx, sy, cz, None if secs is None else secs[sy - base]))
    out.sort(key=lambda s: (s[0], s[2], s[1]))
    return out


def encode(w: world.World, sections) -> bytes:
    """Content (line 2 and the sections) for `sections` of `w`."""
    used = set()
    for *_, s in sections:
        if isinstance(s, int):
            used.add(s)
        elif s is not None:
            used.update(s)
    palette = sorted(used)
    assert len(palette) < UNKNOWN_CELL
    index = {sid: i for i, sid in enumerate(palette)}
    lut = array("H", [UNKNOWN_CELL]) * (max(palette, default=0) + 1)
    for sid, i in index.items():
        lut[sid] = i
    table = world.blocks()
    head = {"min_y": w.min_y, "height": w.height,
            "block_table": {"name": world.DATA.name, "sha256": world.block_table_sha256()},
            "block_tags": world.climb_tags(w.report.block_tags),
            "palette": [table[s].state for s in palette]}
    parts = [json.dumps(head, separators=(",", ":")).encode() + b"\n", struct.pack("<I", len(sections))]
    for sx, sy, sz, s in sections:
        if s is None:
            parts.append(struct.pack("<iiiB", sx, sy, sz, 0))
        elif isinstance(s, int):
            parts.append(struct.pack("<iiiBH", sx, sy, sz, 1, index[s]))
        else:
            cells = array("H", (lut[v] for v in s))
            if sys.byteorder == "big":
                cells.byteswap()
            parts.append(struct.pack("<iiiB", sx, sy, sz, 2) + cells.tobytes())
    return b"".join(parts)


def write_world(path: str | Path, w: world.World, source: str, tick: int | None = None,
                bounds=None, coverage: dict | None = None) -> dict:
    """Write `w` (or the sections meeting `bounds`) to `path`. Returns the header."""
    if bounds is not None:
        bounds = [[min(a, b) for a, b in zip(*bounds)], [max(a, b) for a, b in zip(*bounds)]]
    secs = sections_of(w, bounds)
    content = encode(w, secs)
    header = {"format": FORMAT, "version": VERSION, "source": source, "tick": tick,
              "dimension": w.report.dimension, "dimension_type": w.report.dimension_type,
              "min_y_method": w.min_y_method, "bounds": bounds, "coverage": coverage or {},
              "content_sha256": hashlib.sha256(content).hexdigest()}
    data = json.dumps(header, separators=(",", ":")).encode() + b"\n" + content
    Path(path).write_bytes(gzip.compress(data, compresslevel=9, mtime=0))
    return header


def spans(cols) -> str:
    """'-3: 0-4 7; -2: 5' for chunk columns (x, z)."""
    by_x: dict[int, list[int]] = {}
    for x, z in sorted(cols):
        by_x.setdefault(x, []).append(z)
    out = []
    for x, zs in by_x.items():
        runs, a = [], zs[0]
        for i, z in enumerate(zs):
            if i + 1 == len(zs) or zs[i + 1] != z + 1:
                runs.append(f"{a}" if a == z else f"{a}-{z}")
                a = zs[i + 1] if i + 1 < len(zs) else None
        out.append(f"{x}: {' '.join(runs)}")
    return "; ".join(out)


def tag_check(w: world.World) -> list[str]:
    """Where the server's climbing tags differ from vanilla's (the table's)."""
    tags = world.climb_tags(w.report.block_tags)
    if tags is None:
        return ["the replay has no block tags; the world uses vanilla's climbable / can_glide_through"]
    out = []
    for tag, attr in zip(world.CLIMB_TAGS, ("climbable", "glide_through")):
        server, table = set(tags[tag]), {b.name for b in world.blocks() if getattr(b, attr)}
        if server != table:
            out.append(f"server tag {tag} differs from vanilla (the file has the server's): "
                       f"adds {sorted(server - table)}, lacks {sorted(table - server)}")
    return out


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("replay")
    ap.add_argument("-o", "--out", required=True)
    ap.add_argument("--tick", type=int, help="replay tick (default: the end)")
    ap.add_argument("--bounds", type=int, nargs=6, metavar=("X0", "Y0", "Z0", "X1", "Y1", "Z1"),
                    help="block box, inclusive (default: everything received)")
    args = ap.parse_args(argv)

    r = fb.Replay.load(args.replay, decode_text=False)
    tick = r.total_ticks if args.tick is None else args.tick
    w = world.World.from_replay(r, tick)
    rep = w.report
    bounds = None if args.bounds is None else [args.bounds[:3], args.bounds[3:]]
    if bounds is not None:
        bounds = [[min(a, b) for a, b in zip(*bounds)], [max(a, b) for a, b in zip(*bounds)]]

    print(f"world: {rep.dimension} (dimension type {rep.dimension_type}) at tick {tick}, "
          f"entered at tick {rep.since_tick}")
    vote = "agrees" if rep.min_y_vote == w.min_y else f"says {rep.min_y_vote}"
    flag = "  GUESS" if w.min_y_method == "vote" else ""
    print(f"min_y {w.min_y} ({w.min_y_method}; pose vote {vote}), height {w.height}{flag}")
    secs = sections_of(w, bounds)
    missing = []
    if bounds is None:
        print(f"columns: everything received, {len(w.chunks)}")
    else:
        cols = [(cx, cz) for cx in range(bounds[0][0] >> 4, (bounds[1][0] >> 4) + 1)
                for cz in range(bounds[0][2] >> 4, (bounds[1][2] >> 4) + 1)]
        missing = [c for c in cols if c not in w.chunks]
        print(f"columns: {len(cols) - len(missing)} of {len(cols)} in bounds loaded")
        if missing:
            print(f"  not loaded, chunk x: z spans: {spans(missing)}")
    unknown = sum(s is None for *_, s in secs)
    print(f"sections: {len(secs) - unknown} known, {unknown} unknown")
    print(f"block updates: {rep.updates_applied} applied, {rep.updates_unloaded} dropped (column not loaded, "
          f"as the client drops them), {rep.updates_after} after the tick")
    if rep.forgotten:
        print(f"columns forgotten (now unknown): {rep.forgotten}")
    for line in tag_check(w) or ["server tags climbable, can_glide_through: same as vanilla"]:
        print(line)
    live = {i: e for i, e in rep.entities.items() if world.collides_with_player(e[0])}
    if bounds is not None:
        live = {i: e for i, e in live.items()
                if all(bounds[0][k] - 2 <= e[1 + k] <= bounds[1][k] + 3 for k in range(3))}
    print(f"entities with collision: {len(live)} live at the tick{' in bounds' if bounds else ''}, "
          f"{len(rep.collidable_seen)} seen since entering; not in the world file")
    for i, (name, x, y, z) in sorted(live.items()):
        print(f"  {name} id {i} at {x:.3f} {y:.3f} {z:.3f} (last recorded)")
    coverage = {"updates_applied": rep.updates_applied, "updates_dropped": rep.updates_unloaded,
                "updates_after": rep.updates_after, "columns_missing": len(missing),
                "columns_forgotten": rep.forgotten, "collidable_entities": len(live)}
    h = write_world(args.out, w, r.path.name, tick, bounds, coverage)
    print(f"wrote {args.out}: {Path(args.out).stat().st_size} bytes, content sha256 {h['content_sha256']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
