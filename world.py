"""Blocks of a Flashback replay's world, for collision.

Flashback stores every ClientboundLevelChunkWithLightPacket it saw in
`level_chunk_caches/<n>` (entry i of the replay's chunk list is entry i % 10000
of file i // 10000; each entry is int32 size + the packet with its id), and a
`level_chunk_cached` action references one by index when the client received it.
Block state ids are the 26.2 network ids from data/blocks-26.2.tsv.gz
(tools/block_dump.sh).

`World.from_replay(r, tick)` replays, in stream order up to and including `tick`,
what the client did with: chunk loads (replace a column), BLOCK_UPDATE and
SECTION_BLOCKS_UPDATE (applied; dropped if their column isn't loaded, as the client
drops them), FORGET_LEVEL_CHUNK (the column becomes unknown), and LOGIN / RESPAWN
(a new dimension key clears everything, as `ClientPacketListener.handleRespawn`
makes a new level only then). Flashback 0.43.6 recorded no FORGET_LEVEL_CHUNK in
the four replays checked (2026-10-19 to 2026-11-01), so a column stays as last
received.

Unknown space is explicit: `state()` is None in a column the replay never loaded.
Above and below the dimension's build height it is air (0) everywhere, as in
`LevelChunk.getBlockState`. why? a missing chunk read as air would let a route
through an unloaded wall check out.

`min_y` comes from the dimension type's registry data (configuration phase
REGISTRY_DATA, which EMC sends in full); a vanilla key without data falls back to
vanilla's value; otherwise it is voted from on-ground poses. `World.min_y_method`
says which.

Block table columns (tab-separated; '#' lines are comments, see tools/BlockDump.java
for the `#entity`, `#offset_check` and `#shape_check` lines):
id, state, class, friction, speed, jump, fluid (-/water/lava), boxes
(x0,y0,z0,x1,y1,z1;... block-local, without the position offset, or -), effect
(none/water/lava/bounce/stuck: the block's own class, one per state; climbing is
not in it, see `climb_tags`), offset (none/xz/xyz),
max_h, max_v (the block's max offsets, floats), large (hasLargeCollisionShape),
block_id (block registry id), default (the block's default state), climbable and
glide_through (membership in vanilla's `climbable` / `can_glide_through` tags),
suffocating (`isSuffocating` at an empty origin; see tools/BlockDump.java for what that
misses).
"""

from __future__ import annotations

import gzip
import hashlib
import json
import math
import struct
import zipfile
from array import array
from dataclasses import dataclass, field
from functools import lru_cache
from pathlib import Path

import flashback as fb

DATA = Path(__file__).parent / "data" / "blocks-26.2.tsv.gz"
SECTION = 4096
GLOBAL_BITS = 15  # ceil(log2(number of block states)), checked on load
CONFIG_REGISTRY_DATA = 7  # configuration-phase clientbound ids, protocol 776
CONFIG_UPDATE_TAGS = 13
# Vanilla dimension types, for a registry entry the server sent without data.
VANILLA_MIN_Y = {"minecraft:overworld": (-64, 384), "minecraft:overworld_caves": (-64, 384),
                 "minecraft:the_end": (0, 256), "minecraft:the_nether": (0, 256)}


def f32(v: str) -> float:
    """A Java float's value, widened to double."""
    return struct.unpack("f", struct.pack("f", float(v)))[0]


@dataclass(frozen=True)
class BlockInfo:
    name: str  # "minecraft:oak_stairs"
    state: str  # full state string
    cls: str  # block class, e.g. "SlimeBlock"
    friction: float
    speed: float
    jump: float
    fluid: str  # "-", "water", "lava"
    boxes: tuple[tuple[float, float, float, float, float, float], ...]  # collision AABBs, block-local, un-offset
    effect: str  # "none", "water", "lava", "bounce", "stuck"; climbing comes from tags
    offset: str  # "none", "xz", "xyz"
    max_h: float
    max_v: float
    large: bool
    block_id: int
    default: bool
    climbable: bool
    glide_through: bool
    suffocating: bool


@lru_cache(maxsize=1)
def _table() -> tuple[list[BlockInfo], dict[int, str], str]:
    raw = gzip.decompress(DATA.read_bytes())
    out, entities = [], {}
    for line in raw.decode().splitlines():
        if line.startswith("#entity\t"):
            _, i, name = line.split("\t")
            entities[int(i)] = name
        if line.startswith("#"):
            continue
        (i, state, cls, fr, sp, jp, fluid, boxes, effect, offset, mh, mv, large, bid, default,
         climb, glide, suffocating) = line.split("\t")
        assert int(i) == len(out)
        bx = () if boxes == "-" else tuple(tuple(map(float, b.split(","))) for b in boxes.split(";"))
        out.append(BlockInfo(state.split("[")[0], state, cls, f32(fr), f32(sp), f32(jp), fluid, bx, effect,
                             offset, f32(mh), f32(mv), large == "1", int(bid), default == "1", climb == "1",
                             glide == "1", suffocating == "1"))
    assert (len(out) - 1).bit_length() == GLOBAL_BITS
    return out, entities, hashlib.sha256(raw).hexdigest()


def blocks() -> list[BlockInfo]:
    return _table()[0]


def entity_types() -> dict[int, str]:
    """Entity type registry id -> name ("minecraft:oak_boat")."""
    return _table()[1]


def block_table_sha256() -> str:
    """sha256 of the decompressed block table; world files name the table by it."""
    return _table()[2]


@lru_cache(maxsize=1)
def state_ids() -> dict[str, int]:
    return {b.state: i for i, b in enumerate(blocks())}


CLIMB_TAGS = ("minecraft:climbable", "minecraft:can_glide_through")


def climb_tags(block_tags: dict[str, list[int]] | None) -> dict[str, list[str]] | None:
    """The server's climbable and can_glide_through block tags, as sorted block names;
    a tag the server didn't send is empty, as the client then sees it. None without
    tags (use the table's vanilla columns). why the server's? the client reads tags
    from the server, and a block that is climbable and not glide-through ends a glide
    (LivingEntity.travelFallFlying). EMC sends no can_glide_through (2026-10-19 replay),
    so vines end a glide there."""
    if block_tags is None:
        return None
    names = {b.block_id: b.name for b in blocks()}
    return {t: sorted({names.get(i, f"#{i}") for i in block_tags.get(t, [])}) for t in CLIMB_TAGS}


def collides_with_player(entity_type: str) -> bool:
    """Entity types whose `canBeCollidedWith` is true in 26.2: boats and rafts,
    shulkers, happy ghasts (only while ridden or idle, see HappyGhast). Minecarts don't."""
    n = entity_type.removeprefix("minecraft:")
    return n.endswith(("_boat", "_raft")) or n in ("shulker", "happy_ghast")


# --- chunk packets ---------------------------------------------------------

Section = int | array  # one state for all 4096 cells, or 4096 ids (index y<<8 | z<<4 | x)


def _read_container(buf: fb.Buf, entries: int, small_max: int, small_min_bits: int, global_bits: int):
    """PalettedContainer.read: an int if single-valued, else a list of `entries` global ids."""
    bits = buf.u8()
    if bits == 0:
        return buf.varint()
    if bits <= small_max:
        palette = [buf.varint() for _ in range(buf.varint())]
        bits = max(bits, small_min_bits)
    else:
        assert bits == global_bits, (bits, global_bits)
        palette = None
    per_long = 64 // bits
    n = -(-entries // per_long)
    longs = struct.unpack(f">{n}Q", buf.take(8 * n))
    mask = (1 << bits) - 1
    ids = [longs[i // per_long] >> (i % per_long * bits) & mask for i in range(entries)]
    return [palette[i] for i in ids] if palette is not None else ids


def decode_chunk_packet(data: bytes) -> tuple[int, int, list[Section]]:
    """-> (cx, cz, sections bottom up)."""
    b = fb.Buf(data)
    pid = b.varint()
    assert pid == fb.CB["LEVEL_CHUNK_WITH_LIGHT"], pid
    cx, cz = b.i32(), b.i32()
    for _ in range(b.varint()):  # heightmaps: type, long[]
        b.varint()
        b.take(8 * b.varint())
    size = b.varint()
    sec = fb.Buf(b.take(size))
    sections: list[Section] = []
    while sec.remaining() > 0:
        sec.take(4)  # non-empty block count, fluid count (int16 each)
        ids = _read_container(sec, SECTION, 8, 4, GLOBAL_BITS)
        # biomes: 4x4x4, linear up to 3 bits; we don't need them
        bb = sec.u8()
        if bb == 0:
            sec.varint()
        elif bb <= 3:
            for _ in range(sec.varint()):
                sec.varint()
            sec.take(8 * -(-64 // (64 // bb)))
        else:
            sec.take(8 * -(-64 // (64 // bb)))  # global: width is the wire bits
        sections.append(ids if isinstance(ids, int) else array("H", ids))
    return cx, cz, sections


def _chunk_key(data: bytes) -> tuple[int, int]:
    b = fb.Buf(data)
    assert b.varint() == fb.CB["LEVEL_CHUNK_WITH_LIGHT"]
    return b.i32(), b.i32()


def _cache_entries(data: bytes) -> list[bytes]:
    out, i = [], 0
    while i + 4 <= len(data):
        n = int.from_bytes(data[i:i + 4], "big")
        out.append(data[i + 4:i + 4 + n])
        i += 4 + n
    return out


# --- replay scan -----------------------------------------------------------


@dataclass
class Scan:
    """The replay's actions that matter for blocks, in stream order: (tick, kind, data)."""

    events: list[tuple[int, str, object]] = field(default_factory=list)


def _block_pos(v: int) -> tuple[int, int, int]:
    def sx(u, bits):
        return u - (1 << bits) if u >> (bits - 1) & 1 else u
    v &= 0xFFFFFFFFFFFFFFFF
    return sx(v >> 38, 26), sx(v & 0xFFF, 12), sx(v >> 12 & 0x3FFFFFF, 26)


def _varlong(b: fb.Buf) -> int:
    out = shift = 0
    while True:
        byte = b.u8()
        out |= (byte & 0x7F) << shift
        if not byte & 0x80:
            return out
        shift += 7


def _spawn_info(p: fb.Buf) -> tuple[int, str]:
    """CommonPlayerSpawnInfo head: (dimension type registry id, dimension key)."""
    return p.varint(), p.string()


def _tags(p: fb.Buf) -> dict[str, dict[str, list[int]]]:
    out = {}
    for _ in range(p.varint()):
        reg = p.string()
        out[reg] = {}
        for _ in range(p.varint()):
            name = p.string()
            out[reg][name] = [p.varint() for _ in range(p.varint())]
    return out


@lru_cache(maxsize=4)
def scan_replay(path: Path) -> Scan:
    s = Scan()
    ev = s.events.append
    with zipfile.ZipFile(path) as z:
        meta = json.loads(z.read("metadata.json"))
        if meta.get("protocol_version") != fb.PROTOCOL_VERSION:
            # why? chunk and block packets carry state ids, and the table is 26.2's
            raise ValueError(f"{path}: world.py needs protocol {fb.PROTOCOL_VERSION} for its block "
                             f"table, this replay is {meta.get('protocol_version')}")
        tick = 0
        for name in meta["chunks"]:
            for a in fb._iter_chunk(z.read(name)):
                p = a.payload
                if a.name == "next_tick" and not a.in_snapshot:
                    tick += 1
                elif a.name == "level_chunk_cached":
                    ev((tick, "chunk", p.varint()))
                elif a.name == "move_entities":
                    ev((tick, "move", [(e.id, e.x, e.y, e.z) for e in fb.decode_move_entities(p)]))
                elif a.name == "configuration_packet":
                    pid = p.varint()
                    if pid == CONFIG_REGISTRY_DATA:
                        reg = p.string()
                        if reg == "minecraft:dimension_type":
                            ents = []
                            for _ in range(p.varint()):
                                key = p.string()
                                ents.append((key, fb.read_nbt(p) if p.bool() else None))
                            ev((tick, "dimension_types", ents))
                    elif pid == CONFIG_UPDATE_TAGS:
                        ev((tick, "tags", _tags(p)))
                elif a.name == "game_packet":
                    _scan_game_packet(p, tick, ev)
    return s


def _scan_game_packet(p: fb.Buf, tick: int, ev):
    pid = p.varint()
    if pid == fb.CB["LOGIN"]:
        p.i32()
        p.bool()
        for _ in range(p.varint()):
            p.string()
        p.varint(), p.varint(), p.varint(), p.bool(), p.bool(), p.bool()
        ev((tick, "dimension", _spawn_info(p)))
    elif pid == fb.CB["RESPAWN"]:
        ev((tick, "dimension", _spawn_info(p)))
    elif pid == fb.CB["BLOCK_UPDATE"]:
        ev((tick, "block", (*_block_pos(p.i64()), p.varint())))
    elif pid == fb.CB["SECTION_BLOCKS_UPDATE"]:
        v = p.i64() & 0xFFFFFFFFFFFFFFFF
        sx = (v >> 42) - (1 << 22) if v >> 63 else v >> 42
        sz = v >> 20 & 0x3FFFFF
        sz = sz - (1 << 22) if sz >> 21 else sz
        sy = v & 0xFFFFF
        sy = sy - (1 << 20) if sy >> 19 else sy
        for _ in range(p.varint()):
            c = _varlong(p)
            rel = c & 0xFFF
            ev((tick, "block", (sx * 16 + (rel >> 8 & 15), sy * 16 + (rel & 15), sz * 16 + (rel >> 4 & 15), c >> 12)))
    elif pid == fb.CB["FORGET_LEVEL_CHUNK"]:
        v = p.i64() & 0xFFFFFFFFFFFFFFFF
        cx, cz = v & 0xFFFFFFFF, v >> 32
        ev((tick, "forget", (cx - (1 << 32) if cx >> 31 else cx, cz - (1 << 32) if cz >> 31 else cz)))
    elif pid == fb.CB["ADD_ENTITY"]:
        eid = p.varint()
        p.uuid()
        etype = p.varint()
        ev((tick, "add_entity", (eid, etype, p.f64(), p.f64(), p.f64())))
    elif pid == fb.CB["REMOVE_ENTITIES"]:
        ev((tick, "remove_entities", [p.varint() for _ in range(p.varint())]))
    elif pid == fb.CB["UPDATE_TAGS"]:
        ev((tick, "tags", _tags(p)))


# --- world -----------------------------------------------------------------


@dataclass
class Report:
    """What `from_replay` saw, for the coverage report."""

    dimension: str | None = None
    dimension_type: str | None = None
    since_tick: int = 0  # tick of the dimension change this world starts at
    updates_applied: int = 0
    updates_unloaded: int = 0  # dropped: column not loaded (the client drops these too)
    updates_after: int = 0  # after the chosen tick, in this dimension
    forgotten: int = 0
    entities: dict[int, list] = field(default_factory=dict)  # live at the tick: id -> [type, x, y, z]
    collidable_seen: set[int] = field(default_factory=set)  # collidable entity ids seen since since_tick
    block_tags: dict[str, list[int]] | None = None  # the server's block tags, latest before the tick
    min_y_vote: int | None = None


class World:
    """Block lookup over the chunks a replay received. Coordinates are block ints."""

    def __init__(self, min_y: int, height: int | None = None, min_y_method: str = "given"):
        self.min_y = min_y
        self.height = height  # build height; None: unbounded (hand-made worlds)
        self.min_y_method = min_y_method
        self.chunks: dict[tuple[int, int], list[Section]] = {}
        self.report = Report()

    @classmethod
    def from_replay(cls, r: fb.Replay, tick: int | None = None, min_y: int | None = None,
                    keep_forgotten: bool = False) -> "World":
        """The world as the client had it at the end of `tick` (default: the whole replay).
        `keep_forgotten` keeps columns the client unloaded, with their last blocks. why?
        a long course leaves its start behind, and a check of a whole run needs it."""
        tick = r.total_ticks if tick is None else tick
        scan = scan_replay(Path(r.path))
        w = cls._build(r, scan, tick, min_y, keep_forgotten=keep_forgotten)
        if w.min_y_method == "vote":
            # the vote reads blocks only; updates need the right min_y to land
            vote = w._vote_min_y(r, w.report.since_tick, tick)
            w = cls._build(r, scan, tick, vote, "vote", keep_forgotten=keep_forgotten)
            w.report.min_y_vote = vote
        else:
            w.report.min_y_vote = w._vote_min_y(r, w.report.since_tick, tick, keep=True)
        return w

    @classmethod
    def _build(cls, r, scan: Scan, tick: int, min_y: int | None, method: str = "given",
               keep_forgotten: bool = False) -> "World":
        dim = dim_type = since = start = None
        dim_types: list = []
        for i, (t, kind, data) in enumerate(scan.events):
            if t > tick:
                break
            if kind == "dimension_types":
                dim_types = data
            elif kind == "dimension" and data[1] != dim:
                dim, dim_type, since, start = data[1], data[0], t, i
        if dim is None:
            raise ValueError(f"no LOGIN before tick {tick}")
        type_key, entry = dim_types[dim_type] if dim_type < len(dim_types) else (f"#{dim_type}", None)
        entry = entry or {}
        height = entry.get("height")
        updates = True
        if min_y is None:
            if "min_y" in entry:
                min_y, method = entry["min_y"], "registry"
            elif type_key in VANILLA_MIN_Y:
                (min_y, height), method = VANILLA_MIN_Y[type_key], "vanilla dimension type"
            else:
                min_y, method, updates = 0, "vote", False
        w = cls(min_y, height, method)
        rep = w.report
        rep.dimension, rep.dimension_type, rep.since_tick = dim, type_key, since
        caches: dict[int, list[bytes]] = {}
        pending: dict[tuple[int, int], bytes] = {}
        with zipfile.ZipFile(r.path) as z:
            for t, kind, data in scan.events[start:]:
                if kind == "dimension" and data[1] != dim:
                    break  # left this dimension
                if t > tick:
                    rep.updates_after += kind == "block"
                    continue
                if kind == "chunk":
                    f = data // 10000
                    if f not in caches:
                        caches[f] = _cache_entries(z.read(f"level_chunk_caches/{f}"))
                    raw = caches[f][data % 10000]
                    # decoded only once it is the column's last load or an update hits it
                    key = _chunk_key(raw)
                    pending[key] = raw
                    w.chunks[key] = []
                elif kind == "block" and updates:
                    x, y, z_, sid = data
                    if w.loaded(x, z_):
                        key = (x >> 4, z_ >> 4)
                        if key in pending:
                            w.chunks[key] = decode_chunk_packet(pending.pop(key))[2]
                        w.set(x, y, z_, sid)
                        rep.updates_applied += 1
                    else:
                        rep.updates_unloaded += 1
                elif kind == "forget":
                    if keep_forgotten:
                        continue
                    pending.pop(data, None)
                    if w.chunks.pop(data, None) is not None:
                        rep.forgotten += 1
                elif kind == "add_entity":
                    eid, etype, x, y, z_ = data
                    name = entity_types().get(etype, f"#{etype}")
                    rep.entities[eid] = [name, x, y, z_]
                    if collides_with_player(name):
                        rep.collidable_seen.add(eid)
                elif kind == "move":
                    for eid, x, y, z_ in data:
                        if eid in rep.entities:
                            rep.entities[eid][1:] = [x, y, z_]
                elif kind == "remove_entities":
                    for eid in data:
                        rep.entities.pop(eid, None)
        for key, raw in pending.items():
            w.chunks[key] = decode_chunk_packet(raw)[2]
        if w.height is None and w.chunks:
            w.height = 16 * len(next(iter(w.chunks.values())))
        for t, kind, data in scan.events:
            if t > tick:
                break
            if kind == "tags":
                rep.block_tags = data.get("minecraft:block", rep.block_tags)
        return w

    def _vote_min_y(self, r: fb.Replay, a: int, b: int, keep: bool = False) -> int:
        """The min_y under which the most on-ground poses stand on a block top with no
        block inside the player's box. why both? a world shifted by 16k often still has
        something under a pose, but then also blocks inside it."""
        saved = self.min_y
        poses = [p for t, p in r.poses.items() if a <= t <= b and p.on_ground]
        poses = poses[:: max(1, len(poses) // 400)]
        best = None
        for cand in range(-64, 257, 16):
            self.min_y = cand
            score = sum(self.collidable_below(p.x, p.y, p.z) - 2 * self._inside(p.x, p.y, p.z) for p in poses)
            if best is None or score > best[1]:
                best = (cand, score)
        self.min_y = saved if keep else best[0]
        return best[0]

    def _inside(self, x: float, y: float, z: float) -> bool:
        """A collision box overlaps a standing player's box (0.6 x 1.8) by more than 1e-6."""
        lo, hi = (x - 0.3 + 1e-6, y + 1e-6, z - 0.3 + 1e-6), (x + 0.3 - 1e-6, y + 1.8 - 1e-6, z + 0.3 - 1e-6)
        for bx in range(math.floor(lo[0]), math.floor(hi[0]) + 1):
            for by in range(math.floor(lo[1]), math.floor(hi[1]) + 1):
                for bz in range(math.floor(lo[2]), math.floor(hi[2]) + 1):
                    info = self.info(bx, by, bz)
                    for b in info.boxes if info else ():
                        if (bx + b[0] < hi[0] and bx + b[3] > lo[0] and by + b[1] < hi[1] and by + b[4] > lo[1]
                                and bz + b[2] < hi[2] and bz + b[5] > lo[2]):
                            return True
        return False

    def _section(self, x: int, y: int, z: int) -> tuple[list[Section], int] | None:
        secs = self.chunks.get((x >> 4, z >> 4))
        if secs is None:
            return None
        return secs, (y - self.min_y) >> 4

    def state(self, x: int, y: int, z: int) -> int | None:
        """Block state id, or None for unknown space."""
        if self.height is not None and not self.min_y <= y < self.min_y + self.height:
            return 0
        found = self._section(x, y, z)
        if found is None:
            return None
        secs, si = found
        if si < 0 or si >= len(secs):
            return 0
        s = secs[si]
        return s if isinstance(s, int) else s[((y & 15) << 8) | ((z & 15) << 4) | (x & 15)]

    def info(self, x: int, y: int, z: int) -> BlockInfo | None:
        s = self.state(x, y, z)
        return None if s is None else blocks()[s]

    def loaded(self, x: int, z: int) -> bool:
        return (x >> 4, z >> 4) in self.chunks

    def add_column(self, cx: int, cz: int, fill: int = 0):
        """A loaded column of `fill` over the build height. For hand-made worlds."""
        assert self.height is not None and self.min_y % 16 == 0 and self.height % 16 == 0
        self.chunks[(cx, cz)] = [fill] * (self.height // 16)

    def set(self, x: int, y: int, z: int, state: int | str):
        """Set a block in a loaded column (state id or full state string)."""
        if isinstance(state, str):
            state = state_ids()[state]
        found = self._section(x, y, z)
        assert found is not None, f"column of {x, z} not loaded"
        secs, si = found
        if not 0 <= si < len(secs):
            return  # outside the build height; LevelChunk.setBlockState ignores it too
        s = secs[si]
        if isinstance(s, int):
            if s == state:
                return
            s = secs[si] = array("H", [s]) * SECTION
        s[((y & 15) << 8) | ((z & 15) << 4) | (x & 15)] = state

    def collidable_below(self, x: float, y: float, z: float) -> bool:
        by = math.floor(y - 1e-6)
        for dx in (-0.3, 0.3):
            for dz in (-0.3, 0.3):
                info = self.info(math.floor(x + dx), by, math.floor(z + dz))
                for b in info.boxes if info else ():
                    if abs(by + b[4] - y) < 1e-6:
                        return True
        return False


if __name__ == "__main__":
    import sys
    r = fb.Replay.load(sys.argv[1], decode_text=False)
    w = World.from_replay(r)
    print(f"{len(w.chunks)} chunks in {w.report.dimension} ({w.report.dimension_type}), min_y {w.min_y} "
          f"({w.min_y_method}, vote {w.report.min_y_vote}), height {w.height}, "
          f"updates applied {w.report.updates_applied}, dropped {w.report.updates_unloaded}")
