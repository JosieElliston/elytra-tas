"""Convert an etas-world cache into a Minecraft Java 26.2 singleplayer save.

python3 tools/export_save.py solver/courses/academy/course.json -o /tmp/Academy

Preserves recorded block states, not entities, block entity data (sign text,
inventories), lighting, or EMC plugins. Unknown sections are rejected by default;
--unknown-air explicitly substitutes air. New terrain is void. The save opens in
spectator mode at the course start. No Minecraft session or player is controlled.
Uses the 26.2 split saved-data format, DataVersion 4903.
"""

from __future__ import annotations

import argparse
from collections import defaultdict
import gzip
import hashlib
import json
import math
from pathlib import Path
import struct
import time
import zlib

DATA_VERSION = 4903
REGION_PATH = Path("dimensions/minecraft/overworld/region")


def string(s):
    b = s.encode("utf-8")
    return struct.pack(">H", len(b)) + b


def tag(kind, value):
    return kind, value


def compound(values):
    return tag(10, values)


def payload(t):
    kind, value = t
    if kind in (1, 3, 4, 5, 6):
        return struct.pack({1: ">b", 3: ">i", 4: ">q", 5: ">f", 6: ">d"}[kind], value)
    if kind == 8:
        return string(value)
    if kind == 9:
        element, items = value
        return bytes([element]) + struct.pack(">i", len(items)) + b"".join(payload((element, v)) for v in items)
    if kind == 10:
        return b"".join(bytes([t[0]]) + string(k) + payload(t) for k, t in value.items()) + b"\0"
    if kind in (11, 12):
        fmt = ">i" if kind == 11 else ">Q"
        return struct.pack(">i", len(value)) + b"".join(struct.pack(fmt, v) for v in value)
    raise ValueError(f"unsupported NBT type {kind}")


def nbt(values):
    return b"\x0a\0\0" + payload(compound(values))


def saved_data(path, values):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(gzip.compress(nbt(values), mtime=0))


def state_nbt(state):
    name, _, props = state.partition("[")
    out = {"Name": tag(8, name)}
    if props:
        out["Properties"] = compound({k: tag(8, v) for k, v in
                                      (p.split("=", 1) for p in props.rstrip("]").split(","))})
    return out


def block_states(cells, palette):
    used = sorted(set(cells))
    out = {"palette": tag(9, (10, [state_nbt(palette[i]) for i in used]))}
    if len(used) > 1:
        indices = {v: i for i, v in enumerate(used)}
        bits = max(4, (len(used) - 1).bit_length())
        per_long = 64 // bits
        words = []
        for start in range(0, 4096, per_long):
            words.append(sum(indices[v] << (bits * j) for j, v in enumerate(cells[start:start + per_long])))
        out["data"] = tag(12, words)
    return compound(out)


def read_world(path, unknown_air=False):
    raw = gzip.decompress(path.read_bytes())
    header_raw, content = raw.split(b"\n", 1)
    header = json.loads(header_raw)
    if (header.get("format"), header.get("version")) != ("etas-world", 1):
        raise ValueError("expected etas-world version 1")
    if hashlib.sha256(content).hexdigest() != header["content_sha256"]:
        raise ValueError("world content checksum mismatch")
    head_raw, data = content.split(b"\n", 1)
    head = json.loads(head_raw)
    palette = head["palette"]
    if "minecraft:air" not in palette:
        palette.append("minecraft:air")
    air = palette.index("minecraft:air")
    columns = defaultdict(dict)
    pos = 4
    unknown = 0
    for _ in range(struct.unpack_from("<I", data)[0]):
        sx, sy, sz, kind = struct.unpack_from("<iiiB", data, pos)
        pos += 13
        if kind == 0:
            cells = [air] * 4096
            unknown += 4096
        elif kind == 1:
            cells = [struct.unpack_from("<H", data, pos)[0]] * 4096
            pos += 2
        elif kind == 2:
            cells = list(struct.unpack_from("<4096H", data, pos))
            pos += 8192
            unknown += cells.count(65535)
            cells = [air if v == 65535 else v for v in cells]
        else:
            raise ValueError(f"invalid section kind {kind}")
        if (sx, sz) in columns and sy in columns[sx, sz]:
            raise ValueError("duplicate section")
        columns[sx, sz][sy] = cells
    if pos != len(data):
        raise ValueError("trailing world data")
    if unknown and not unknown_air:
        raise ValueError(f"{unknown} unknown cells; use --unknown-air to substitute air")
    # An omitted vertical section is also unknown in etas-world.
    expected = set(range(head["min_y"] // 16, (head["min_y"] + head["height"]) // 16))
    if any(set(secs) != expected for secs in columns.values()):
        raise ValueError("world must contain every vertical section in each column")
    return header, head, columns, unknown


def chunk_bytes(cx, cz, sections, palette, min_y):
    secs = [{"Y": tag(1, sy), "block_states": block_states(cells, palette),
             "biomes": compound({"palette": tag(9, (8, ["minecraft:plains"]))})}
            for sy, cells in sorted(sections.items())]
    return nbt({"DataVersion": tag(3, DATA_VERSION), "xPos": tag(3, cx), "zPos": tag(3, cz),
                "yPos": tag(3, min_y // 16), "Status": tag(8, "minecraft:full"),
                "LastUpdate": tag(4, 0), "InhabitedTime": tag(4, 0),
                "isLightOn": tag(1, 0), "sections": tag(9, (10, secs)),
                "block_entities": tag(9, (10, [])), "block_ticks": tag(9, (10, [])),
                "fluid_ticks": tag(9, (10, [])),
                "PostProcessing": tag(9, (9, [(3, []) for _ in secs])),
                "structures": compound({"starts": compound({}), "References": compound({})})})


def write_regions(out, columns, head):
    regions = defaultdict(list)
    for (cx, cz), secs in sorted(columns.items()):
        raw = chunk_bytes(cx, cz, secs, head["palette"], head["min_y"])
        compressed = zlib.compress(raw)
        record = struct.pack(">I", len(compressed) + 1) + b"\x02" + compressed
        regions[cx // 32, cz // 32].append((cx, cz, record))
    region_dir = out / REGION_PATH
    region_dir.mkdir(parents=True)
    for (rx, rz), chunks in regions.items():
        locations = bytearray(4096)
        records = []
        offset = 2
        for cx, cz, record in chunks:
            sectors = math.ceil(len(record) / 4096)
            if sectors > 255:
                raise ValueError("chunk too large for inline Anvil storage")
            i = (cx % 32 + (cz % 32) * 32) * 4
            locations[i:i + 4] = struct.pack(">I", offset << 8 | sectors)
            records.append(record.ljust(sectors * 4096, b"\0"))
            offset += sectors
        (region_dir / f"r.{rx}.{rz}.mca").write_bytes(locations + bytes(4096) + b"".join(records))


def write_metadata(out, name, pose):
    x, y, z, yaw, pitch = pose
    saved_data(out / "level.dat", {"Data": compound({
        "DataVersion": tag(3, DATA_VERSION), "version": tag(3, 19133),
        "Version": compound({"Id": tag(3, DATA_VERSION), "Name": tag(8, "26.2"),
                             "Series": tag(8, "main"), "Snapshot": tag(1, 0)}),
        "LevelName": tag(8, name), "GameType": tag(3, 3), "allowCommands": tag(1, 1),
        "initialized": tag(1, 1), "Time": tag(4, 0), "LastPlayed": tag(4, int(time.time() * 1000)),
        "difficulty_settings": compound({"difficulty": tag(8, "peaceful"),
                                          "locked": tag(1, 0), "hardcore": tag(1, 0)}),
        "spawn": compound({"pos": tag(11, [math.floor(x), math.floor(y), math.floor(z)]),
                           "yaw": tag(5, yaw), "pitch": tag(5, pitch),
                           "dimension": tag(8, "minecraft:overworld")}),
        "DataPacks": compound({"Enabled": tag(9, (8, ["vanilla"])), "Disabled": tag(9, (8, []))}),
    })})
    data_dir = out / "data" / "minecraft"
    def data_file(name, values):
        saved_data(data_dir / f"{name}.dat", {"DataVersion": tag(3, DATA_VERSION), "data": compound(values)})
    settings = compound({"features": tag(1, 0), "lakes": tag(1, 0),
                         "layers": tag(9, (10, [{"block": tag(8, "minecraft:air"), "height": tag(3, 1)}])),
                         "structure_overrides": tag(9, (8, [])), "biome": tag(8, "minecraft:the_void")})
    dimensions = {"minecraft:overworld": compound({
        "type": tag(8, "minecraft:overworld"), "generator": compound({
            "type": tag(8, "minecraft:flat"), "settings": settings})})}
    # Vanilla's stable world preset requires all three standard dimensions.
    for dimension, biome_source, noise in (("the_nether", "multi_noise", "nether"),
                                            ("the_end", "the_end", "end")):
        biome = {"type": tag(8, f"minecraft:{biome_source}")}
        if dimension == "the_nether":
            biome["preset"] = tag(8, "minecraft:nether")
        dimensions[f"minecraft:{dimension}"] = compound({
            "type": tag(8, f"minecraft:{dimension}"), "generator": compound({
                "type": tag(8, "minecraft:noise"), "biome_source": compound(biome),
                "settings": tag(8, f"minecraft:{noise}")})})
    data_file("world_gen_settings", {"seed": tag(4, 0), "generate_structures": tag(1, 0),
        "bonus_chest": tag(1, 0), "dimensions": compound(dimensions)})
    data_file("game_rules", {"minecraft:advance_time": tag(1, 0), "minecraft:advance_weather": tag(1, 0),
                            "minecraft:spawn_mobs": tag(1, 0), "minecraft:random_tick_speed": tag(3, 0),
                            "minecraft:fire_spread_radius_around_player": tag(3, 0),
                            "minecraft:respawn_radius": tag(3, 0)})
    data_file("world_clocks", {"minecraft:overworld": compound({"total_ticks": tag(4, 6000)})})


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("course", type=Path)
    ap.add_argument("-o", "--out", type=Path, required=True)
    ap.add_argument("--name", help="name in the singleplayer world list")
    ap.add_argument("--unknown-air", action="store_true")
    args = ap.parse_args()
    course = json.loads(args.course.read_text())
    source = args.course.parent / course["world"]["path"]
    header, head, columns, unknown = read_world(source, args.unknown_air)
    if head["min_y"] != -64 or head["height"] != 384:
        raise ValueError("this exporter requires the standard overworld build height")
    if header["content_sha256"] != course["world"]["content_sha256"]:
        raise ValueError("course/world checksum mismatch")
    pose = [struct.unpack(">d" if i < 3 else ">f", bytes.fromhex(v))[0]
            for i, v in enumerate(course["start"]["pose"])]
    args.out.mkdir(parents=True, exist_ok=False)
    write_regions(args.out, columns, head)
    write_metadata(args.out, args.name or f"{course['name'].title()} (recorded blocks)", pose)
    report = {"source": str(source.resolve()), "content_sha256": header["content_sha256"],
              "columns": len(columns), "sections": sum(map(len, columns.values())),
              "unknown_cells_replaced": unknown, "start_pose": pose,
              "limitations": ["No entities or block entity data", "No EMC course logic",
                              "Unrecorded terrain is void", "Vanilla physics and block tags"]}
    (args.out / "etas-export.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    print(f"Wrote {args.out}")


if __name__ == "__main__":
    main()
