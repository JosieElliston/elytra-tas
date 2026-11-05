"""Every player a Flashback replay saw, not only the local one.

usage: python3 tools/players.py REPLAY.zip [--dimension minecraft:eacademy]    # list them

`load(path)` -> `Recording`: one `Life` per spawn of a player entity (the local player's
is one life per dimension visit), with what Flashback kept of it. What each kind holds:

- **Local player**: `move_entities` poses every tick it moved, exact client doubles and
  rotation (yaw wrapped; see flashback.py). Fly these with `extract_run.py`.
- **Remote players and EMC's replay NPCs**: what the server's entity tracker sent. A
  player entity's tracker runs every 2 server ticks, so poses come about every 2 client
  ticks, x/y/z on the 1/4096 grid of the move packets (Flashback records the target, not
  the client's lerp) and rotation in steps of 360/256 degrees. `syncs` are the
  ENTITY_POSITION_SYNC packets, which carry exact doubles, velocity and float rotation
  (the tracker sends them when onGround changes among others). `motions` are
  SET_ENTITY_MOTION, LpVec3-quantized.

`Recording.replay_status` is EMC's replay viewer line per tick ("Playing 0:12 / 0:34 1x",
sent as system chat), since a watched NPC can be paused, rewound or slowed, and
`Recording.replays` the "Playing replay: NAME" lines.
"""

from __future__ import annotations

import argparse
import json
import re
import zipfile
from dataclasses import dataclass, field
from pathlib import Path

import flashback as fb

# minecraft:player in the entity type registry, by protocol (26.2: data/blocks-26.2.tsv.gz)
PLAYER_TYPE = {776: 156}
GLIDE_BIT = 1 << fb.FLAG_GLIDE
STATUS = re.compile(r"(Playing|Paused)\s+(\d+):(\d+)(?:\.\d+)?\s*/\s*(\d+):(\d+)(?:\.\d+)?\s+([\d.]+)x")


@dataclass
class Life:
    """One spawn of a player entity, up to its removal or the end of the dimension visit."""

    id: int
    uuid: str | None
    local: bool
    dimension: str | None
    first_tick: int
    last_tick: int = -1
    poses: dict[int, fb.EntityPos] = field(default_factory=dict)
    motions: list[tuple[int, tuple[float, float, float]]] = field(default_factory=list)
    # (tick, x, y, z, vx, vy, vz, yaw, pitch)
    syncs: list[tuple] = field(default_factory=list)
    flags: list[tuple[int, int]] = field(default_factory=list)  # (tick, shared flags byte)


@dataclass
class ReplayStatus:
    playing: bool
    at: int  # seconds into the replay
    total: int  # seconds
    speed: float


@dataclass
class Recording:
    path: Path
    meta: dict
    total_ticks: int = 0
    lives: list[Life] = field(default_factory=list)
    names: dict[str, str] = field(default_factory=dict)  # uuid -> profile name
    local_uuid: str | None = None
    dims: list[tuple[int, str]] = field(default_factory=list)  # (tick, dimension) on change
    chats: list[tuple[int, str]] = field(default_factory=list)  # all but replay status lines
    replay_status: dict[int, ReplayStatus] = field(default_factory=dict)
    replays: list[tuple[int, str]] = field(default_factory=list)  # (tick, replay name)

    def name_of(self, life: Life) -> str:
        return self.names.get(life.uuid, f"?{(life.uuid or str(life.id))[:8]}")


def _player_info(p: fb.Buf, names: dict[str, str]):
    """ClientboundPlayerInfoUpdatePacket: an action bitset, then per entry each action's
    fields in action order. ADD_PLAYER is the first, so the rest needn't be decoded."""
    actions = p.u8()
    if not actions & 1:
        return
    for _ in range(p.varint()):
        uuid = p.uuid()
        names[uuid] = p.string()
        for _ in range(p.varint()):  # profile properties
            p.string(), p.string()
            if p.bool():
                p.string()
        if actions & 2 and p.bool():  # INITIALIZE_CHAT
            p.uuid(), p.i64()
            p.take(p.varint()), p.take(p.varint())
        if actions & 4:  # UPDATE_GAME_MODE
            p.varint()
        if actions & 8:  # UPDATE_LISTED
            p.bool()
        if actions & 16:  # UPDATE_LATENCY
            p.varint()
        if actions & 32 and p.bool():  # UPDATE_DISPLAY_NAME
            fb.read_nbt(p)
        if actions & 64:  # UPDATE_LIST_ORDER
            p.varint()
        if actions & 128:  # UPDATE_HAT
            p.bool()


def load(path: str | Path) -> Recording:
    path = Path(path)
    with zipfile.ZipFile(path) as z:
        meta = json.loads(z.read("metadata.json"))
        pid_map = fb.packet_ids(meta)
        rec = Recording(path, meta)
        live: dict[int, Life] = {}
        tick, local_id, dim = 0, None, None
        player_type = PLAYER_TYPE.get(meta["protocol_version"])
        uuid_types: dict[str, int] = {}

        def end(life: Life):
            life.last_tick = max(life.poses, default=life.first_tick)
            rec.lives.append(life)

        def set_dim(d):
            nonlocal dim
            if d != dim:
                for life in live.values():
                    end(life)
                live.clear()
                dim = d
                rec.dims.append((tick, d))

        def local_life():
            if local_id is not None and local_id not in live:
                live[local_id] = Life(local_id, rec.local_uuid, True, dim, tick)
            return live.get(local_id)

        for chunk in meta["chunks"]:
            for a in fb._iter_chunk(z.read(chunk)):
                p = a.payload
                if a.name == "next_tick":
                    tick += not a.in_snapshot
                elif a.name == "create_local_player":
                    rec.local_uuid = p.uuid()
                elif a.name == "move_entities":
                    for e in fb.decode_move_entities(p):
                        if e.id == local_id:
                            local_life()
                        if e.id in live and not a.in_snapshot:
                            live[e.id].poses[tick] = e
                elif a.name == "game_packet":
                    pid = pid_map[p.varint()]
                    if pid == fb.CB["LOGIN"]:
                        local_id = p.i32()
                        p.bool()
                        for _ in range(p.varint()):
                            p.string()
                        p.varint(), p.varint(), p.varint(), p.bool(), p.bool(), p.bool()
                        p.varint()
                        set_dim(p.string())
                    elif pid == fb.CB["RESPAWN"]:
                        p.varint()
                        set_dim(p.string())
                    elif pid == fb.CB["PLAYER_INFO_UPDATE"]:
                        _player_info(p, rec.names)
                    elif pid == fb.CB["ADD_ENTITY"]:
                        eid = p.varint()
                        uuid = p.uuid()
                        etype = p.varint()
                        # 1.21.11's type ids aren't in our table: a tab-list uuid says player
                        is_player = etype == player_type or (player_type is None and uuid in rec.names)
                        if is_player and (eid not in live or live[eid].uuid != uuid):
                            if eid in live:
                                end(live.pop(eid))
                            live[eid] = Life(eid, uuid, False, dim, tick)
                    elif pid == fb.CB["REMOVE_ENTITIES"] and not a.in_snapshot:
                        for eid in [p.varint() for _ in range(p.varint())]:
                            if eid in live and eid != local_id:
                                end(live.pop(eid))
                    elif pid == fb.CB["SET_ENTITY_MOTION"] and not a.in_snapshot:
                        eid = p.varint()
                        if eid in live:
                            live[eid].motions.append((tick, fb.read_lpvec3(p)))
                    elif pid == fb.CB["ENTITY_POSITION_SYNC"] and not a.in_snapshot:
                        eid = p.varint()
                        if eid in live:
                            v = [p.f64() for _ in range(6)]
                            live[eid].syncs.append((tick, *v, p.f32(), p.f32()))
                    elif pid == fb.CB["SET_ENTITY_DATA"] and not a.in_snapshot:
                        eid = p.varint()
                        if eid in live and p.u8() == 0 and p.varint() == 0:
                            live[eid].flags.append((tick, p.u8()))
                    elif pid == fb.CB["SYSTEM_CHAT"] and not a.in_snapshot:
                        try:
                            text = fb.component_text(fb.read_nbt(p))
                        except Exception:  # noqa: BLE001 - keep going on odd components
                            continue
                        m = STATUS.fullmatch(text.strip())
                        if m:
                            rec.replay_status[tick] = ReplayStatus(
                                m[1] == "Playing", int(m[2]) * 60 + int(m[3]), int(m[4]) * 60 + int(m[5]), float(m[6]))
                        else:
                            rec.chats.append((tick, text))
                            if text.startswith("Playing replay: "):
                                rec.replays.append((tick, text.removeprefix("Playing replay: ").strip()))
        rec.total_ticks = tick
        for life in live.values():
            end(life)
    rec.lives.sort(key=lambda x: x.first_tick)
    return rec


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("replay")
    ap.add_argument("--dimension")
    ap.add_argument("--min-poses", type=int, default=20)
    args = ap.parse_args(argv)
    rec = load(args.replay)
    print(f"{rec.path.name}: {rec.total_ticks} ticks, protocol {rec.meta['protocol_version']}, "
          f"local {rec.names.get(rec.local_uuid, rec.local_uuid)}")
    for t, name in rec.replays:
        print(f"  replay @{t}: {name}")
    for life in rec.lives:
        if (args.dimension and life.dimension != args.dimension) or len(life.poses) < args.min_poses:
            continue
        print(f"  {rec.name_of(life):<18} {'local' if life.local else 'remote':<6} {life.dimension} "
              f"ticks {life.first_tick}-{life.last_tick} poses {len(life.poses)} "
              f"syncs {len(life.syncs)} motions {len(life.motions)}")


if __name__ == "__main__":
    main()
