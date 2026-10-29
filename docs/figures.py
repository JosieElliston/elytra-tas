"""Draws the tick-timing figures in docs/: python3 docs/figures.py

Times are in ms on a 50 ms server tick, with a server tick's work W = 18 ms (about what
EMC showed on 2026-10-28). Edit the numbers here and rerun rather than editing the SVGs.
"""

from pathlib import Path

W_PX = 900
X0 = 110          # x of t = 0
PX = 5            # px per ms
SERVER_TICKS = (10, 60, 110)
WORK = 18
CLIENT_WORK = 6

# Dark, with the background drawn in, so the figure looks the same in a README, a PNG or a
# chat preview. Plain colors, not CSS variables: rsvg and some image previews don't support var().
BACKGROUND = "#1f1e1d"
STYLE = """<style>
text{font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Helvetica,Arial,sans-serif;font-size:13px;fill:#e8e6df;dominant-baseline:central}
.h{font-size:15px;font-weight:600}
.m{fill:#b4b2a9}
.c{text-anchor:middle}
.box{fill:#3a3936}
.brk{fill:none;stroke:#b4b2a9;stroke-width:1}
.pk{fill:none;stroke-width:1.75}
.wait{fill:none;stroke-width:1.75;stroke-dasharray:5 4}
.move{stroke:#afa9ec}.reply{stroke:#5dcaa5}.ping{stroke:#b4b2a9}
.dot-move{fill:#afa9ec}.dot-reply{fill:#5dcaa5}
</style>"""


def defs():
    out = ["<defs>"]
    for kind in ("move", "reply", "ping"):
        out.append(f'<marker id="a-{kind}" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" '
                   f'orient="auto-start-reverse"><path d="M1 1L9 5L1 9" fill="none" class="{kind}" '
                   f'stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></marker>')
    out.append("</defs>")
    return "\n".join(out)


def x(t):
    return X0 + PX * t


class Panel:
    """One timeline: a title, two rows of brackets, the server row, the client row."""

    def __init__(self, y0, title, server_labels=("tick N−1", "tick N", "tick N+1"), client_ticks=(0, 50, 100),
                 server_ticks=None):
        self.y0 = y0
        self.s0, self.s1 = y0 + 62, y0 + 94      # server boxes
        self.c0, self.c1 = y0 + 154, y0 + 176    # client boxes
        self.parts = [f'<text class="h" x="20" y="{y0}">{title}</text>',
                      f'<text class="m" x="20" y="{(self.s0 + self.s1) / 2}">server</text>',
                      f'<text class="m" x="20" y="{(self.c0 + self.c1) / 2}">client</text>']
        for (t, work), label in zip(server_ticks or [(t, WORK) for t in SERVER_TICKS], server_labels):
            self.parts.append(f'<rect class="box" x="{x(t)}" y="{self.s0}" width="{PX * work}" height="{self.s1 - self.s0}"/>')
            self.parts.append(f'<text class="c" x="{x(t + work / 2)}" y="{(self.s0 + self.s1) / 2}">{label}</text>')
        for t in client_ticks:
            self.parts.append(f'<rect class="box" x="{x(t)}" y="{self.c0}" width="{PX * CLIENT_WORK}" height="{self.c1 - self.c0}"/>')

    def up(self, kind, t_send, t_arrive):
        self.parts.append(f'<line class="pk {kind}" x1="{x(t_send)}" y1="{self.c0}" x2="{x(t_arrive)}" y2="{self.s1}" '
                          f'marker-end="url(#a-{kind})"/>')

    def down(self, kind, t_send, t_arrive):
        self.parts.append(f'<line class="pk {kind}" x1="{x(t_send)}" y1="{self.s1}" x2="{x(t_arrive)}" y2="{self.c0}" '
                          f'marker-end="url(#a-{kind})"/>')

    # Waits run along the edge packets arrive at, so each arrow's tip meets the next one's tail.
    def server_wait(self, kind, t0, t1, dot=None):
        y = self.s1
        self.parts.append(f'<line class="wait {kind}" x1="{x(t0)}" y1="{y}" x2="{x(t1)}" y2="{y}" marker-end="url(#a-{kind})"/>')
        if dot is not None:
            self.parts.append(f'<circle cx="{x(dot)}" cy="{y}" r="3" class="dot-{kind}"/>')

    def client_wait(self, kind, t0, t1):
        y = self.c0
        self.parts.append(f'<line class="wait {kind}" x1="{x(t0)}" y1="{y}" x2="{x(t1)}" y2="{y}" marker-end="url(#a-{kind})"/>')

    def bracket(self, row, t0, t1, label):
        y = self.y0 + (30 if row == 2 else 54)
        self.parts.append(f'<path class="brk" d="M{x(t0)} {y + 5}V{y}H{x(t1)}V{y + 5}"/>')
        self.parts.append(f'<text class="c" x="{(x(t0) + x(t1)) / 2}" y="{y - 9}">{label}</text>')

    def bracket_below(self, t0, t1, label):
        y = self.c1 + 10
        self.parts.append(f'<path class="brk" d="M{x(t0)} {y - 5}V{y}H{x(t1)}V{y - 5}"/>')
        self.parts.append(f'<text class="c" x="{(x(t0) + x(t1)) / 2}" y="{y + 12}">{label}</text>')

    def arrow_label(self, t_bottom, t_top, label):
        """A label beside the midpoint of a packet line, on the side the line leans away from."""
        mid, y = (x(t_bottom) + x(t_top)) / 2, (self.c0 + self.s1) / 2 + 8
        if t_top > t_bottom:
            self.parts.append(f'<text x="{mid + 12}" y="{y}">{label}</text>')
        else:
            self.parts.append(f'<text x="{mid - 12}" y="{y}" style="text-anchor:end">{label}</text>')

    def note(self, row, t, label):
        y = self.y0 + (30 if row == 2 else 54)
        self.parts.append(f'<text class="c" x="{x(t)}" y="{y - 9}">{label}</text>')

    def text(self, t, y, label, cls="m"):
        self.parts.append(f'<text class="{cls}" x="{x(t)}" y="{y}">{label}</text>')

    def svg(self):
        return "\n".join(self.parts)


def legend(y, items):
    out = []
    for i, (kind, dashed, label) in enumerate(items):
        yy = y + 22 * i
        cls = "wait" if dashed else "pk"
        out.append(f'<line class="{cls} {kind}" x1="20" y1="{yy}" x2="60" y2="{yy}"/>')
        out.append(f'<text x="72" y="{yy}">{label}</text>')
    return "\n".join(out)


def document(height, body, title, desc):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W_PX} {height}" width="{W_PX}" height="{height}" '
            f'role="img"><title>{title}</title><desc>{desc}</desc>\n{STYLE}\n{defs()}\n'
            f'<rect width="100%" height="100%" fill="{BACKGROUND}"/>\n{body}\n</svg>\n')


def ticks():
    out = []

    # 1. The loop, and its partition into six intervals.
    p = Panel(30, "A move and its reply: one loop, six intervals")
    send, arrive = CLIENT_WORK, 34          # net up 28
    reply_at = 60 + WORK                    # flushed at the end of tick N
    back = 90                               # net down 12
    p.up("move", send, arrive)
    p.server_wait("move", arrive, 60)
    p.down("move", reply_at, back)
    p.client_wait("move", back, 100)
    p.arrow_label(send, arrive, "net up")
    p.arrow_label(back, reply_at, "net down")
    p.bracket(1, arrive, 60, "server wait")
    p.bracket(1, 60, reply_at, "server work")
    p.bracket_below(0, CLIENT_WORK, "client work")
    p.bracket_below(back, 100, "client wait")
    out.append(p.svg())

    # 2. A move arriving mid-tick.
    p = Panel(290, "A move arriving mid-tick", client_ticks=(27, 77, 127))
    p.up("move", 33, 65)
    p.server_wait("move", 65, 110, dot=60 + WORK)
    p.bracket(2, 65, 110, "server wait")
    p.bracket(1, 65, 60 + WORK, "queued")
    p.bracket(1, 60 + WORK, 110, "waits for tick N+1")
    out.append(p.svg())

    # 3. Replies that don't wait for a tick.
    p = Panel(530, "Replies sent without a tick: tab completion and the debug ping")
    p.up("reply", 8, 32)
    p.down("reply", 32, 44)
    p.note(1, 32, "idle: answered at once")
    p.up("reply", 46, 70)
    p.server_wait("reply", 70, 60 + WORK)
    p.down("reply", 60 + WORK, 90)
    p.bracket(1, 70, 60 + WORK, "busy: waits for the tick's end")
    p.up("ping", 92, 116)
    p.down("ping", 116, 128)
    p.note(1, 116, "network thread: at once")
    out.append(p.svg())

    out.append(legend(750, [
        ("move", False, "read by a tick; its reply leaves when that tick ends: move, sprint toggle"),
        ("reply", False, "answered by its handler between ticks: tab completion, chat command"),
        ("ping", False, "answered on the network thread, even mid-tick: debug ping"),
        ("ping", True, "dashes: waiting in a queue"),
    ]))
    return document(830, "\n".join(out), "Server and client ticks",
                    "Packets between client and server ticks: a move and its reply split into six intervals, "
                    "a move arriving mid-tick, and replies that don't wait for a tick.")


def ebounce():
    out = []
    p = Panel(30, "Normal bounce: the landing reaches tick N", server_labels=("tick N−1", "glide off", "tick N+1"),
              client_ticks=(0, 50))
    p.up("move", CLIENT_WORK, 34)
    p.server_wait("move", 34, 60)
    p.up("move", 50 + CLIENT_WORK, 84)
    p.server_wait("move", 84, 110)
    p.text(CLIENT_WORK - 6, (p.c0 + p.s1) / 2 + 16, "land", "m")
    p.text(50 + CLIENT_WORK - 6, (p.c0 + p.s1) / 2 + 16, "jump", "m")
    p.note(1, 45, "tick N sees you on the ground")
    out.append(p.svg())

    p = Panel(270, "Perfect bounce, close arrivals: land just misses tick N, jump beats tick N+1",
              server_labels=("tick N−1", "tick N", "still gliding"), client_ticks=(30, 80))
    p.up("move", 30 + CLIENT_WORK, 62)
    p.server_wait("move", 62, 110, dot=60 + WORK)
    p.up("move", 80 + CLIENT_WORK, 108)
    p.text(30 + CLIENT_WORK - 6, (p.c0 + p.s1) / 2 + 16, "land", "m")
    p.text(80 + CLIENT_WORK - 6, (p.c0 + p.s1) / 2 + 16, "jump", "m")
    p.bracket(1, 62, 60 + WORK, "just missed tick N")
    p.note(2, 109, "jump arrives before N+1 starts")
    out.append(p.svg())

    # A lag spike: tick N runs long, the next one starts late to catch up.
    spike = 65
    p = Panel(510, "Perfect bounce, lag spike: both moves queue behind one long tick",
              server_labels=("tick N−1", "tick N (lag spike)", "still gliding"), client_ticks=(30, 80),
              server_ticks=[(10, WORK), (60, spike), (60 + spike + 0.6, WORK)])  # 3 px gap: no outlines
    p.up("move", 30 + CLIENT_WORK, 62)
    p.server_wait("move", 62, 60 + spike + 0.6)
    p.up("move", 80 + CLIENT_WORK, 110)
    p.text(30 + CLIENT_WORK - 6, (p.c0 + p.s1) / 2 + 16, "land", "m")
    p.text(80 + CLIENT_WORK - 6, (p.c0 + p.s1) / 2 + 16, "jump", "m")
    p.bracket(1, 62, 60 + spike, "both queued, then handled together")
    out.append(p.svg())

    out.append(legend(730, [
        ("move", False, "move packet: applied when the server handles it, checked by the next tick"),
        ("move", True, "dashes: waiting for a tick"),
    ]))
    return document(770, "\n".join(out), "Normal and perfect ebounce",
                    "A normal bounce, where the landing move reaches tick N and the glide ends, and two perfect "
                    "bounces: the landing just misses tick N and the jump arrives before tick N+1, or both "
                    "moves queue behind a long tick and are handled together.")


if __name__ == "__main__":
    here = Path(__file__).parent
    (here / "ticks.svg").write_text(ticks())
    (here / "ebounce.svg").write_text(ebounce())
