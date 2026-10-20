r"""Reader for the elytra-tas mod's network/tick logs (mod/, `NetLog.java`).

One gzipped TSV per server connection, at
`<gameDir>/elytra-tas/logs/<yyyy-MM-dd_HH-mm-ss>.tsv.gz`. Line 1 is
`#elytra-tas-netlog<TAB>1`; every other line is one record:

    type  t  tick  fields...

`t` is System.nanoTime() (one monotonic clock for all threads), `tick` the number
of the most recent START_CLIENT_TICK in the session (0 before the first). Doubles
are Java's shortest round-trip decimal; floats are widened to double first, so
`float()` gives the exact value and `numpy.float32()` of it is the exact float.
Booleans are 0/1, a missing value is `-`, text escapes `\\ \t \n \r`.

    M  wall_ms key=value...            session start; wall-clock epoch ms sampled at t
    T  s|e x y z yRot xRot vx vy vz onGround hColl fallFlying sprinting input
                                       local player at START/END_CLIENT_TICK; input is
                                       Input bits: 1 fwd 2 back 4 left 8 right 16 jump
                                       32 shift 64 sprint
    S  m|n seq name details...         Connection.send called (thread m=client, n=Netty)
    W  seq bytes                       that packet's final frame reached the socket end
                                       of the pipeline (seq -1: not matched to a packet)
    F  t_end                           channel flush from t to t_end; the socket write
                                       happens inside it
    R  rseq bytes                      raw socket read (before frame splitting)
    D  seq rseq name                   inbound packet decoded (Netty), rseq = latest read
    H  m|n seq name details...         inbound packet handler starts: m = client thread
                                       (queued; once per frame, also between ticks),
                                       n = inline on Netty (keep_alive). seq -1 = the
                                       bundle wrapper, its sub-packets follow
    X  event server_tick x y z vx vy vz onGround fallFlying extra...
                                       integrated server, local player only
    E  event args...                   logger events: stop <reason>, dropped <n>

S details: move_player_*: x y z yRot xRot onGround hColl; player_command: action
data; player_input: input; keep_alive / pong / accept_teleportation: id.
H details: player_position: id x y z vx vy vz yRot xRot relatives;
set_entity_motion / set_entity_data: only for the local player, `self` then vx vy vz
/ `id=value;...` (bytes as 0x.., id 0 is the shared flags, 0x80 = gliding);
keep_alive / ping: id; system_chat: overlay text; set_action_bar_text /
set_title_text / set_subtitle_text: text; respawn: dimension gameType dataToKeep;
login: dimension gameType playerId.
X events: tick_start / tick_end (ServerPlayer.doTick), move (handleMovePlayer done;
extra = packet name, packet onGround, hColl), command (handlePlayerCommand; action),
glide_end (extra = updateFallFlying or stopFallFlying), start_refused (START_FALL_FLYING
refused while not gliding; builds from c59e227 log these as glide_end stopFallFlying).

Facts that bite:
- D/H pairs are joined by seq; the reader does it (`Log.inbound`). S/W likewise
  (`Log.outbound`). Records from different threads are not in time order in the
  file; sort by t.
- the client handles queued packets at the start of every frame, not every tick, so
  an H can land between two ticks' T records. Compare times, not tick numbers.
- X move records can't be joined to S by seq: the integrated server decodes its own
  copy. They are in send order.
"""

from __future__ import annotations

import bisect
import gzip
import sys
import zlib
from collections import Counter
from dataclasses import dataclass, field
from pathlib import Path
from typing import Iterator

FORMAT = ("#elytra-tas-netlog", "1")
_UNESCAPE = {"\\": "\\", "t": "\t", "n": "\n", "r": "\r"}

MOVE = ("move_player_pos", "move_player_pos_rot", "move_player_rot", "move_player_status_only")
FLAG_GLIDING = 0x80  # shared flags (entity data id 0) bit 7


def unescape(s: str) -> str:
    if "\\" not in s:
        return s
    out, i = [], 0
    while i < len(s):
        c = s[i]
        if c == "\\" and i + 1 < len(s):
            out.append(_UNESCAPE.get(s[i + 1], s[i + 1]))
            i += 2
        else:
            out.append(c)
            i += 1
    return "".join(out)


def opt_float(s: str) -> float | None:
    return None if s == "-" else float(s)


@dataclass(frozen=True)
class Rec:
    kind: str
    t: int
    tick: int
    f: tuple[str, ...]  # fields after tick, unescaped


@dataclass(frozen=True)
class Tick:
    t: int
    tick: int
    phase: str  # "s" start, "e" end
    pos: tuple[float, float, float]
    yrot: float
    xrot: float
    vel: tuple[float, float, float]
    on_ground: bool
    hcoll: bool
    fall_flying: bool
    sprinting: bool
    input: int


@dataclass
class Outbound:
    seq: int
    name: str
    t_send: int
    tick: int
    thread: str
    details: tuple[str, ...]
    t_wire: int | None = None  # W
    bytes: int | None = None
    t_flushed: int | None = None  # end of the first flush after t_wire

    def move(self) -> dict | None:
        """x y z yRot xRot (None when absent), on_ground, hcoll; None if not a move packet."""
        if self.name not in MOVE:
            return None
        d = self.details
        return dict(
            x=opt_float(d[0]), y=opt_float(d[1]), z=opt_float(d[2]),
            yrot=opt_float(d[3]), xrot=opt_float(d[4]),
            on_ground=d[5] == "1", hcoll=d[6] == "1",
        )


@dataclass
class Inbound:
    seq: int
    name: str
    t_decode: int
    rseq: int
    t_read: int | None = None  # R with rseq
    t_handle: int | None = None  # H
    handle_tick: int | None = None
    thread: str | None = None
    details: tuple[str, ...] = ()


@dataclass(frozen=True)
class Server:
    t: int
    tick: int
    event: str
    server_tick: int
    pos: tuple[float, float, float]
    vel: tuple[float, float, float]
    on_ground: bool
    fall_flying: bool
    extra: tuple[str, ...]


@dataclass
class Log:
    path: Path
    meta: dict[str, str] = field(default_factory=dict)
    wall_ms: int = 0
    t0: int = 0  # nanoTime of the M record
    ticks: list[Tick] = field(default_factory=list)
    outbound: list[Outbound] = field(default_factory=list)
    inbound: list[Inbound] = field(default_factory=list)
    server: list[Server] = field(default_factory=list)
    events: list[Rec] = field(default_factory=list)
    counts: Counter = field(default_factory=Counter)
    truncated: bool = False

    def wall(self, t: int) -> float:
        """Epoch ms of nanoTime t."""
        return self.wall_ms + (t - self.t0) / 1e6

    def ms(self, t: int) -> float:
        """ms since session start."""
        return (t - self.t0) / 1e6


def _records(path: str | Path) -> Iterator[Rec]:
    """Raises EOFError at the end of a file cut off by a crash."""
    with gzip.open(path, "rt", encoding="utf-8", newline="\n") as f:
        header = f.readline().rstrip("\n").split("\t")
        if tuple(header) != FORMAT:
            raise ValueError(f"{path}: not an elytra-tas netlog v1 (header {header!r})")
        for line in f:
            if not line.endswith("\n"):
                raise EOFError("partial last line")
            parts = line[:-1].split("\t")
            yield Rec(parts[0], int(parts[1]), int(parts[2]), tuple(unescape(p) for p in parts[3:]))


def records(path: str | Path) -> Iterator[Rec]:
    """Raw records in file order. A file cut off by a crash ends early, cleanly."""
    try:
        yield from _records(path)
    except (EOFError, zlib.error):
        return


def _b(s: str) -> bool:
    return s == "1"


def load(path: str | Path) -> Log:
    log = Log(Path(path))
    out: dict[int, Outbound] = {}
    inb: dict[int, Inbound] = {}
    reads: dict[int, int] = {}
    flushes: list[tuple[int, int]] = []
    wires: list[tuple[int, int, int]] = []
    handles: list[Rec] = []
    try:
        for r in _records(path):
            log.counts[r.kind] += 1
            f = r.f
            if r.kind == "M":
                log.wall_ms, log.t0 = int(f[0]), r.t
                log.meta = dict(kv.split("=", 1) for kv in f[1:])
            elif r.kind == "T":
                log.ticks.append(Tick(
                    r.t, r.tick, f[0], (float(f[1]), float(f[2]), float(f[3])), float(f[4]), float(f[5]),
                    (float(f[6]), float(f[7]), float(f[8])), _b(f[9]), _b(f[10]), _b(f[11]), _b(f[12]), int(f[13])))
            elif r.kind == "S":
                o = Outbound(int(f[1]), f[2], r.t, r.tick, f[0], f[3:])
                out[o.seq] = o
                log.outbound.append(o)
            elif r.kind == "W":
                wires.append((int(f[0]), r.t, int(f[1])))
            elif r.kind == "F":
                flushes.append((r.t, int(f[0])))
            elif r.kind == "R":
                reads[int(f[0])] = r.t
            elif r.kind == "D":
                i = Inbound(int(f[0]), f[2], r.t, int(f[1]))
                inb[i.seq] = i
                log.inbound.append(i)
            elif r.kind == "H":
                handles.append(r)
            elif r.kind == "X":
                log.server.append(Server(
                    r.t, r.tick, f[0], int(f[1]), (float(f[2]), float(f[3]), float(f[4])),
                    (float(f[5]), float(f[6]), float(f[7])), _b(f[8]), _b(f[9]), f[10:]))
            else:
                log.events.append(r)
    except (EOFError, zlib.error):
        log.truncated = True

    flushes.sort()
    starts = [s for s, _ in flushes]
    for seq, t, n in wires:
        o = out.get(seq)
        if o is None:
            continue
        o.t_wire, o.bytes = t, n
        k = bisect.bisect_left(starts, t)
        if k < len(flushes):
            o.t_flushed = flushes[k][1]
    for i in log.inbound:
        i.t_read = reads.get(i.rseq)
    for r in handles:
        i = inb.get(int(r.f[1]))
        if i is not None:
            i.t_handle, i.handle_tick, i.thread, i.details = r.t, r.tick, r.f[0], r.f[3:]
    log.ticks.sort(key=lambda k: k.t)
    log.outbound.sort(key=lambda o: o.t_send)
    log.inbound.sort(key=lambda i: i.t_decode)
    log.server.sort(key=lambda x: x.t)
    return log


def _show(fields: tuple[str, ...]) -> str:
    return " ".join(f.replace("\\", "\\\\").replace("\t", "\\t").replace("\n", "\\n") for f in fields)


def timeline(log: Log, everything: bool = False) -> Iterator[tuple[int, str]]:
    """(t, text) for ticks, outbound packets, described inbound packets, and server events,
    in time order. `everything` adds name-only inbound packets."""
    rows: list[tuple[int, str]] = []
    for k in log.ticks:
        flags = "G" * k.on_ground + "H" * k.hcoll + "F" * k.fall_flying + "S" * k.sprinting
        rows.append((k.t, f"T{k.phase} {k.tick:6d} y={k.pos[1]:.6f} vy={k.vel[1]:+.6f} {flags or '-'} in={k.input}"))
    for o in log.outbound:
        wire = "" if o.t_wire is None else f" wire+{(o.t_wire - o.t_send) / 1e3:.0f}us"
        rows.append((o.t_send, f"S  {o.tick:6d} {o.name} {_show(o.details)}{wire}"))
    for i in log.inbound:
        if i.t_handle is None or (not i.details and not everything):
            continue
        lat = f" decode->handle {(i.t_handle - i.t_decode) / 1e6:.2f}ms"
        rows.append((i.t_handle, f"H{i.thread} {i.handle_tick:6d} {i.name} {_show(i.details)}{lat}"))
    for x in log.server:
        rows.append((x.t, f"X  {x.tick:6d} {x.event} st={x.server_tick} y={x.pos[1]:.6f} "
                          f"g={int(x.on_ground)} ff={int(x.fall_flying)} {_show(x.extra)}"))
    rows.sort(key=lambda r: r[0])
    return iter(rows)


def main(argv: list[str]) -> int:
    import argparse

    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("log")
    ap.add_argument("--timeline", action="store_true", help="print a merged time-ordered view")
    ap.add_argument("--all", action="store_true", help="with --timeline, include name-only inbound packets")
    args = ap.parse_args(argv)

    log = load(args.log)
    print(f"{log.path.name}: {' '.join(f'{k}={v}' for k, v in log.meta.items())}")
    if log.truncated:
        print("  (file truncated; read up to the last complete flush)")
    print(f"  records: {' '.join(f'{k}={n}' for k, n in sorted(log.counts.items()))}")
    if log.ticks:
        print(f"  ticks {log.ticks[0].tick}..{log.ticks[-1].tick}, "
              f"{(log.ticks[-1].t - log.ticks[0].t) / 1e9:.1f} s")
    lat = sorted(o.t_wire - o.t_send for o in log.outbound if o.t_wire is not None and o.name in MOVE)
    if lat:
        print(f"  move send->wire: median {lat[len(lat) // 2] / 1e3:.0f} us, max {lat[-1] / 1e3:.0f} us")
    hq = sorted(i.t_handle - i.t_decode for i in log.inbound if i.t_handle is not None and i.thread == "m")
    if hq:
        print(f"  inbound decode->handle (client thread): median {hq[len(hq) // 2] / 1e6:.2f} ms, "
              f"max {hq[-1] / 1e6:.2f} ms")
    for e in log.events:
        print(f"  {e.kind} {log.ms(e.t):.1f}ms {' '.join(e.f)}")
    if args.timeline:
        for t, text in timeline(log, args.all):
            print(f"{log.ms(t):12.3f} {text}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
