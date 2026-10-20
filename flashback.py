"""Reader for Flashback (Moulberry/Flashback) replay archives.

A replay zip holds metadata.json plus chunk files c0.flashback, c1.flashback, ...
Each chunk file is:

    int32 magic (0xD780E884)
    varint n; n * identifier          -- action-name table, index = action id
    int32 snapshot_size; snapshot     -- actions that rebuild state at chunk start
    actions...                        -- until EOF

and each action is `varint id, int32 size, payload[size]`. The format was read
off Flashback 0.43.6's ReplayReader / Recorder; game packet IDs are for MC 26.2
(protocol 776) and were read off GameProtocols in the client jar.

Facts about what gets recorded (Recorder.endTick, Flashback 0.43.6):
- once per client tick, at end of tick: `move_entities` with exact doubles
  x/y/z and floats yaw/pitch/headYaw/onGround for every entity whose position
  changed, *including the local player*; then a SetEntityMotion game packet for
  the local player (lossy LpVec3 encoding, don't use it for physics); then
  `next_tick`.
- inbound server packets are recorded as `game_packet` actions as they arrive.
- keypresses are NOT recorded. Inputs must be reconstructed from the trajectory.
"""

from __future__ import annotations

import io
import json
import struct
import zipfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Iterator

MAGIC = 0xD780E884
PROTOCOL_VERSION = 776  # MC 26.2

# Clientbound play packet IDs, protocol 776. Index = packet id.
CLIENTBOUND = """BUNDLE ADD_ENTITY ANIMATE AWARD_STATS BLOCK_CHANGED_ACK BLOCK_DESTRUCTION
BLOCK_ENTITY_DATA BLOCK_EVENT BLOCK_UPDATE BOSS_EVENT CHANGE_DIFFICULTY CHUNK_BATCH_FINISHED
CHUNK_BATCH_START CHUNKS_BIOMES CLEAR_TITLES COMMAND_SUGGESTIONS COMMANDS CONTAINER_CLOSE
CONTAINER_SET_CONTENT CONTAINER_SET_DATA CONTAINER_SET_SLOT COOKIE_REQUEST COOLDOWN
CUSTOM_CHAT_COMPLETIONS CUSTOM_PAYLOAD DAMAGE_EVENT DEBUG_BLOCK_VALUE DEBUG_CHUNK_VALUE
DEBUG_ENTITY_VALUE DEBUG_EVENT DEBUG_SAMPLE DELETE_CHAT DISCONNECT DISGUISED_CHAT
ENTITY_EVENT ENTITY_POSITION_SYNC EXPLODE FORGET_LEVEL_CHUNK GAME_EVENT GAME_RULE_VALUES
GAME_TEST_HIGHLIGHT_POS MOUNT_SCREEN_OPEN HURT_ANIMATION INITIALIZE_BORDER KEEP_ALIVE
LEVEL_CHUNK_WITH_LIGHT LEVEL_EVENT LEVEL_PARTICLES LIGHT_UPDATE LOGIN LOW_DISK_SPACE_WARNING
MAP_ITEM_DATA MERCHANT_OFFERS MOVE_ENTITY_POS MOVE_ENTITY_POS_ROT MOVE_MINECART_ALONG_TRACK
MOVE_ENTITY_ROT MOVE_VEHICLE OPEN_BOOK OPEN_SCREEN OPEN_SIGN_EDITOR PING PONG_RESPONSE
PLACE_GHOST_RECIPE PLAYER_ABILITIES PLAYER_CHAT PLAYER_COMBAT_END PLAYER_COMBAT_ENTER
PLAYER_COMBAT_KILL PLAYER_INFO_REMOVE PLAYER_INFO_UPDATE PLAYER_LOOK_AT PLAYER_POSITION
PLAYER_ROTATION RECIPE_BOOK_ADD RECIPE_BOOK_REMOVE RECIPE_BOOK_SETTINGS REMOVE_ENTITIES
REMOVE_MOB_EFFECT RESET_SCORE RESOURCE_PACK_POP RESOURCE_PACK_PUSH RESPAWN ROTATE_HEAD
SECTION_BLOCKS_UPDATE SELECT_ADVANCEMENTS_TAB SERVER_DATA SET_ACTION_BAR_TEXT
SET_BORDER_CENTER SET_BORDER_LERP_SIZE SET_BORDER_SIZE SET_BORDER_WARNING_DELAY
SET_BORDER_WARNING_DISTANCE SET_CAMERA SET_CHUNK_CACHE_CENTER SET_CHUNK_CACHE_RADIUS
SET_CURSOR_ITEM SET_DEFAULT_SPAWN_POSITION SET_DISPLAY_OBJECTIVE SET_ENTITY_DATA
SET_ENTITY_LINK SET_ENTITY_MOTION SET_EQUIPMENT SET_EXPERIENCE SET_HEALTH SET_HELD_SLOT
SET_OBJECTIVE SET_PASSENGERS SET_PLAYER_INVENTORY SET_PLAYER_TEAM SET_SCORE
SET_SIMULATION_DISTANCE SET_SUBTITLE_TEXT SET_TIME SET_TITLE_TEXT SET_TITLES_ANIMATION
SOUND_ENTITY SOUND START_CONFIGURATION STOP_SOUND STORE_COOKIE SYSTEM_CHAT TAB_LIST
TAG_QUERY TAKE_ITEM_ENTITY TELEPORT_ENTITY TEST_INSTANCE_BLOCK_STATUS TICKING_STATE
TICKING_STEP TRANSFER UPDATE_ADVANCEMENTS UPDATE_ATTRIBUTES UPDATE_MOB_EFFECT
UPDATE_RECIPES UPDATE_TAGS PROJECTILE_POWER CUSTOM_REPORT_DETAILS SERVER_LINKS WAYPOINT
CLEAR_DIALOG SHOW_DIALOG""".split()
CB = {name: i for i, name in enumerate(CLIENTBOUND)}


class Buf:
    """Big-endian Minecraft-style byte reader over a bytes object."""

    __slots__ = ("b", "i", "end")

    def __init__(self, b: bytes, i: int = 0, end: int | None = None):
        self.b, self.i = b, i
        self.end = len(b) if end is None else end

    def remaining(self) -> int:
        return self.end - self.i

    def take(self, n: int) -> bytes:
        if self.i + n > self.end:
            raise EOFError(f"want {n} bytes at {self.i}, end {self.end}")
        out = self.b[self.i : self.i + n]
        self.i += n
        return out

    def u8(self) -> int:
        v = self.b[self.i]
        self.i += 1
        return v

    def bool(self) -> bool:
        return self.u8() != 0

    def i32(self) -> int:
        return struct.unpack(">i", self.take(4))[0]

    def u32(self) -> int:
        return struct.unpack(">I", self.take(4))[0]

    def i64(self) -> int:
        return struct.unpack(">q", self.take(8))[0]

    def f32(self) -> float:
        return struct.unpack(">f", self.take(4))[0]

    def f64(self) -> float:
        return struct.unpack(">d", self.take(8))[0]

    def varint(self) -> int:
        out = shift = 0
        while True:
            byte = self.u8()
            out |= (byte & 0x7F) << shift
            if not byte & 0x80:
                break
            shift += 7
        if out & (1 << 31):
            out -= 1 << 32
        return out

    def string(self) -> str:
        return self.take(self.varint()).decode("utf-8")

    def uuid(self) -> str:
        h = self.take(16).hex()
        return f"{h[:8]}-{h[8:12]}-{h[12:16]}-{h[16:20]}-{h[20:]}"

    def sub(self, n: int) -> "Buf":
        s = Buf(self.b, self.i, self.i + n)
        self.i += n
        return s


# --- NBT (network form: root tag has a type byte but no name) --------------


def read_nbt(buf: Buf):
    t = buf.u8()
    return None if t == 0 else _nbt_payload(buf, t)


def _nbt_payload(buf: Buf, t: int):
    if t == 1:
        return struct.unpack(">b", buf.take(1))[0]
    if t == 2:
        return struct.unpack(">h", buf.take(2))[0]
    if t == 3:
        return buf.i32()
    if t == 4:
        return buf.i64()
    if t == 5:
        return buf.f32()
    if t == 6:
        return buf.f64()
    if t == 7:
        return buf.take(buf.i32())
    if t == 8:
        return buf.take(struct.unpack(">H", buf.take(2))[0]).decode("utf-8", "replace")
    if t == 9:
        et = buf.u8()
        return [_nbt_payload(buf, et) for _ in range(buf.i32())]
    if t == 10:
        out = {}
        while (ct := buf.u8()) != 0:
            name = buf.take(struct.unpack(">H", buf.take(2))[0]).decode("utf-8", "replace")
            out[name] = _nbt_payload(buf, ct)
        return out
    if t == 11:
        return [buf.i32() for _ in range(buf.i32())]
    if t == 12:
        return [buf.i64() for _ in range(buf.i32())]
    raise ValueError(f"bad nbt tag {t}")


def component_text(c) -> str:
    """Flatten a text component (as decoded NBT) to plain text."""
    if c is None:
        return ""
    if isinstance(c, str):
        return c
    if isinstance(c, list):
        return "".join(component_text(x) for x in c)
    if isinstance(c, dict):
        s = c.get("text", c.get("", ""))
        if not isinstance(s, str):
            s = str(s)
        if "translate" in c:
            s += c["translate"]
            if "with" in c:
                s += "(" + ", ".join(component_text(x) for x in c["with"]) + ")"
        for x in c.get("extra", []):
            s += component_text(x)
        return s
    return str(c)


# --- actions ---------------------------------------------------------------


@dataclass
class EntityPos:
    id: int
    x: float
    y: float
    z: float
    yaw: float
    pitch: float
    head_yaw: float
    on_ground: bool


@dataclass
class Action:
    name: str  # without the "flashback:action/" prefix
    payload: Buf
    in_snapshot: bool


def _iter_chunk(data: bytes) -> Iterator[Action]:
    buf = Buf(data)
    if buf.u32() != MAGIC:
        raise ValueError("not a flashback chunk (bad magic)")
    names = []
    for _ in range(buf.varint()):
        n = buf.string()
        names.append(n.removeprefix("flashback:action/"))
    snap_size = buf.i32()
    snap_end = buf.i + snap_size
    while buf.remaining() > 0:
        in_snap = buf.i < snap_end
        aid = buf.varint()
        size = buf.i32()
        yield Action(names[aid], buf.sub(size), in_snap)


def decode_move_entities(p: Buf) -> list[EntityPos]:
    out = []
    for _ in range(p.varint()):
        p.string()  # dimension key
        for _ in range(p.varint()):
            out.append(
                EntityPos(p.varint(), p.f64(), p.f64(), p.f64(), p.f32(), p.f32(), p.f32(), p.bool())
            )
    return out


@dataclass
class PlayerPosition:
    """ClientboundPlayerPositionPacket: a server teleport of the local player."""

    teleport_id: int
    x: float
    y: float
    z: float
    dx: float
    dy: float
    dz: float
    yaw: float
    pitch: float
    relatives: int  # bitset of Relative flags

    @classmethod
    def decode(cls, p: Buf) -> "PlayerPosition":
        return cls(p.varint(), p.f64(), p.f64(), p.f64(), p.f64(), p.f64(), p.f64(), p.f32(), p.f32(), p.i32())


@dataclass
class Event:
    """A recorded thing of interest, stamped with the tick it happened in."""

    tick: int
    kind: str
    data: object


@dataclass
class Replay:
    path: Path
    metadata: dict
    local_id: int | None = None
    local_uuid: str | None = None
    # tick -> local player pose at end of that tick (missing ticks: unchanged)
    poses: dict[int, EntityPos] = field(default_factory=dict)
    events: list[Event] = field(default_factory=list)
    packet_counts: dict[str, int] = field(default_factory=dict)
    total_ticks: int = 0

    @classmethod
    def load(cls, path: str | Path, decode_text: bool = True) -> "Replay":
        path = Path(path)
        with zipfile.ZipFile(path) as z:
            meta = json.loads(z.read("metadata.json"))
            if meta.get("protocol_version") != PROTOCOL_VERSION:
                raise ValueError(
                    f"replay is protocol {meta.get('protocol_version')}, parser knows {PROTOCOL_VERSION}"
                )
            r = cls(path, meta)
            tick = 0
            for chunk_name in meta["chunks"]:
                for a in _iter_chunk(z.read(chunk_name)):
                    r._handle(a, tick, decode_text)
                    if a.name == "next_tick" and not a.in_snapshot:
                        tick += 1
            r.total_ticks = tick
        return r

    def _handle(self, a: Action, tick: int, decode_text: bool):
        if a.name == "move_entities":
            for e in decode_move_entities(a.payload):
                if e.id == self.local_id:
                    self.poses[tick] = e
        elif a.name == "create_local_player":
            self.local_uuid = a.payload.uuid()
        elif a.name == "game_packet":
            self._game_packet(a.payload, tick, a.in_snapshot, decode_text)

    def _game_packet(self, p: Buf, tick: int, in_snap: bool, decode_text: bool):
        pid = p.varint()
        name = CLIENTBOUND[pid] if pid < len(CLIENTBOUND) else f"#{pid}"
        self.packet_counts[name] = self.packet_counts.get(name, 0) + 1
        if pid == CB["LOGIN"]:
            self.local_id = p.i32()
            self.events.append(Event(tick, "login", self.local_id))
        elif in_snap:
            return
        elif pid == CB["PLAYER_POSITION"]:
            self.events.append(Event(tick, "teleport", PlayerPosition.decode(p)))
        elif pid == CB["RESPAWN"]:
            self.events.append(Event(tick, "respawn", None))
        elif decode_text and pid in (CB["SYSTEM_CHAT"], CB["SET_TITLE_TEXT"], CB["SET_SUBTITLE_TEXT"], CB["SET_ACTION_BAR_TEXT"]):
            try:
                text = component_text(read_nbt(p))
            except Exception as ex:  # noqa: BLE001 - keep going on odd components
                text = f"<undecodable: {ex}>"
            kind = {CB["SYSTEM_CHAT"]: "chat", CB["SET_TITLE_TEXT"]: "title",
                    CB["SET_SUBTITLE_TEXT"]: "subtitle", CB["SET_ACTION_BAR_TEXT"]: "actionbar"}[pid]
            self.events.append(Event(tick, kind, text))
        elif pid == CB["SET_HELD_SLOT"]:
            self.events.append(Event(tick, "held_slot", p.varint()))
        elif pid == CB["SET_ENTITY_DATA"]:
            if p.varint() != self.local_id:
                return
            # Items are (u8 index, varint serializer, value), 0xff-terminated.
            # Only index 0 (shared flags, serializer 0 = byte) is decoded; it is
            # first when present because Flashback writes items in index order.
            if p.u8() == 0 and p.varint() == 0:
                self.events.append(Event(tick, "flags", p.u8()))


# Entity shared-flag bits (Entity.FLAG_*)
FLAG_ON_FIRE, FLAG_SHIFT, FLAG_SPRINT, FLAG_SWIM, FLAG_INVISIBLE, FLAG_GLOW, FLAG_GLIDE = 0, 1, 3, 4, 5, 6, 7
