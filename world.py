"""Blocks of a Flashback replay's world, for collision.

Flashback stores every ClientboundLevelChunkWithLightPacket it saw in
`level_chunk_caches/<n>` (entry i of the replay's chunk list is entry i % 10000
of file i // 10000; each entry is int32 size + the packet with its id), and a
`level_chunk_cached` action references one by index when the client received it.
Block state ids are the 26.2 network ids from data/blocks-26.2.tsv.gz
(tools/block_dump.sh).

A course is its own dimension on EMC, and a replay can visit several. World holds
the chunks received after the replay's last dimension change before `tick`;
block updates (BLOCK_UPDATE / SECTION_BLOCKS_UPDATE) are not applied. why? the
courses we fly are static, and a missed update shows up as a collision mismatch in
the physics check, not silently.
"""

from __future__ import annotations

import gzip
import json
import zipfile
from dataclasses import dataclass
from functools import lru_cache
from pathlib import Path

import flashback as fb

DATA = Path(__file__).parent / "data" / "blocks-26.2.tsv.gz"
SECTION = 4096
GLOBAL_BITS = 15  # ceil(log2(number of block states)), checked on load


@dataclass(frozen=True)
class BlockInfo:
    name: str  # "minecraft:oak_stairs"
    state: str  # full state string
    cls: str  # block class, e.g. "SlimeBlock"
    friction: float
    speed: float
    jump: float
    fluid: str  # "-", "water", "lava"
    boxes: tuple[tuple[float, float, float, float, float, float], ...]  # collision AABBs, block-local


@lru_cache(maxsize=1)
def blocks() -> list[BlockInfo]:
    out = []
    with gzip.open(DATA, "rt") as f:
        for line in f:
            if line.startswith("#"):
                continue
            i, state, cls, fr, sp, jp, fluid, boxes = line.rstrip("\n").split("\t")
            assert int(i) == len(out)
            bx = () if boxes == "-" else tuple(tuple(map(float, b.split(","))) for b in boxes.split(";"))
            out.append(BlockInfo(state.split("[")[0], state, cls, float(fr), float(sp), float(jp), fluid, bx))
    assert (len(out) - 1).bit_length() == GLOBAL_BITS
    return out


def _bits_storage(buf: fb.Buf, bits: int) -> list[int]:
    per_long = 64 // bits
    n = -(-SECTION // per_long) if bits else 0
    longs = [buf.i64() & 0xFFFFFFFFFFFFFFFF for _ in range(n)]
    mask = (1 << bits) - 1
    out = []
    for v in longs:
        for k in range(per_long):
            out.append(v >> (k * bits) & mask)
    return out[:SECTION]


def _read_container(buf: fb.Buf, entries: int, small_max: int, small_min_bits: int, global_bits: int | None):
    """PalettedContainer.read. Returns a list of `entries` global ids, or None if the
    container is global with unknown width (biomes; we only skip those)."""
    bits = buf.u8()
    if bits == 0:
        v = buf.varint()
        return [v] * entries
    if bits <= small_max:
        palette = [buf.varint() for _ in range(buf.varint())]
        bits = max(bits, small_min_bits)
        per_long = 64 // bits
        n = -(-entries // per_long)
        longs = [buf.i64() & 0xFFFFFFFFFFFFFFFF for _ in range(n)]
        mask = (1 << bits) - 1
        return [palette[longs[i // per_long] >> (i % per_long * bits) & mask] for i in range(entries)]
    if global_bits is None:
        raise ValueError(f"global palette with {bits} bits and unknown registry size")
    assert bits == global_bits, (bits, global_bits)
    per_long = 64 // bits
    n = -(-entries // per_long)
    longs = [buf.i64() & 0xFFFFFFFFFFFFFFFF for _ in range(n)]
    mask = (1 << bits) - 1
    return [longs[i // per_long] >> (i % per_long * bits) & mask for i in range(entries)]


def decode_chunk_packet(data: bytes):
    """-> (cx, cz, [section block-id lists, bottom up; None for an all-air section])."""
    b = fb.Buf(data)
    pid = b.varint()
    assert pid == fb.CB["LEVEL_CHUNK_WITH_LIGHT"], pid
    cx, cz = b.i32(), b.i32()
    for _ in range(b.varint()):  # heightmaps: type, long[]
        b.varint()
        b.take(8 * b.varint())
    size = b.varint()
    sec = fb.Buf(b.take(size))
    sections = []
    while sec.remaining() > 0:
        nonempty = struct_short(sec)
        struct_short(sec)  # fluid count
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
        sections.append(None if nonempty == 0 else ids)
    return cx, cz, sections


def struct_short(b: fb.Buf) -> int:
    v = int.from_bytes(b.take(2), "big", signed=True)
    return v


class World:
    """Block lookup over the chunks a replay received. Coordinates are block ints."""

    def __init__(self, min_y: int):
        self.min_y = min_y
        self.chunks: dict[tuple[int, int], list] = {}

    @classmethod
    def from_replay(cls, r: fb.Replay, tick: int | None = None, min_y: int | None = None) -> "World":
        """Chunks received before `tick` (default: the whole replay), after the last
        respawn (dimension change) before it."""
        tick = r.total_ticks if tick is None else tick
        respawns = [e.tick for e in r.events if e.kind == "respawn" and e.tick <= tick]
        since = respawns[-1] if respawns else -1
        refs = [i for t, i in r.chunk_refs if since <= t <= tick]
        chunks = {}
        with zipfile.ZipFile(r.path) as z:
            files: dict[int, list[bytes]] = {}
            for i in refs:
                f = i // 10000
                if f not in files:
                    files[f] = _cache_entries(z.read(f"level_chunk_caches/{f}"))
                cx, cz, secs = decode_chunk_packet(files[f][i % 10000])
                chunks[(cx, cz)] = secs
        w = cls(0 if min_y is None else min_y)
        w.chunks = chunks
        if min_y is None:
            w.min_y = w._guess_min_y(r, since, tick)
        return w

    def _guess_min_y(self, r: fb.Replay, a: int, b: int) -> int:
        """The dimension's min_y comes from registry data we don't parse; pick the
        candidate that puts a collidable block under every on-ground pose."""
        poses = [p for t, p in r.poses.items() if a <= t <= b and p.on_ground]
        best = None
        for cand in range(-64, 257, 16):
            self.min_y = cand
            hits = sum(1 for p in poses[:: max(1, len(poses) // 400)]
                       if self.collidable_below(p.x, p.y, p.z))
            if best is None or hits > best[1]:
                best = (cand, hits)
        self.min_y = best[0]
        return best[0]

    def state(self, x: int, y: int, z: int) -> int:
        secs = self.chunks.get((x >> 4, z >> 4))
        if secs is None:
            return 0
        si = (y - self.min_y) >> 4
        if si < 0 or si >= len(secs) or secs[si] is None:
            return 0
        return secs[si][((y & 15) << 8) | ((z & 15) << 4) | (x & 15)]

    def info(self, x: int, y: int, z: int) -> BlockInfo:
        return blocks()[self.state(x, y, z)]

    def loaded(self, x: int, z: int) -> bool:
        return (x >> 4, z >> 4) in self.chunks

    def collidable_below(self, x: float, y: float, z: float) -> bool:
        import math
        by = math.floor(y - 1e-6)
        for dx in (-0.3, 0.3):
            for dz in (-0.3, 0.3):
                info = self.info(math.floor(x + dx), by, math.floor(z + dz))
                for b in info.boxes:
                    if abs(by + b[4] - y) < 1e-6:
                        return True
        return False


def _cache_entries(data: bytes) -> list[bytes]:
    out, i = [], 0
    while i + 4 <= len(data):
        n = int.from_bytes(data[i:i + 4], "big")
        out.append(data[i + 4:i + 4 + n])
        i += 4 + n
    return out


if __name__ == "__main__":
    import sys
    r = fb.Replay.load(sys.argv[1], decode_text=False)
    w = World.from_replay(r)
    print(f"{len(w.chunks)} chunks, min_y {w.min_y}, sections/chunk {set(len(s) for s in w.chunks.values())}")
