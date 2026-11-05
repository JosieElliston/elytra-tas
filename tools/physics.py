"""Client movement physics from MC 26.2, for validating extracted inputs.

Elytra: LivingEntity.updateFallFlyingMovement. Float/double usage mirrors the
Java so that, without collisions, a tick reproduces the recorded displacement
to ~1e-14 (double rounding in the position subtraction, not model error).
"""

from __future__ import annotations

import math

import numpy as np

f32 = np.float32
_SIN = np.array([math.sin(i / 10430.378350470453) for i in range(65536)], dtype=np.float32)
_D2R = f32(math.pi / 180.0)


def mth_sin(x: float) -> float:
    return float(_SIN[int(x * 10430.378350470453) & 65535])


def mth_cos(x: float) -> float:
    return float(_SIN[int(x * 10430.378350470453 + 16384.0) & 65535])


def look_angle(xrot: float, yrot: float) -> tuple[float, float, float]:
    """Entity.calculateViewVector."""
    rx = float(f32(xrot) * _D2R)
    ry = float(-f32(yrot) * _D2R)
    yc, ys, xc, xs = (f32(v) for v in (mth_cos(ry), mth_sin(ry), mth_cos(rx), mth_sin(rx)))
    return float(ys * xc), float(-xs), float(yc * xc)


def elytra_step(v: tuple[float, float, float], yaw: float, pitch: float, gravity: float = 0.08):
    """Velocity after one gliding tick (before collisions)."""
    lx, ly, lz = look_angle(pitch, yaw)
    lean = float(f32(pitch) * _D2R)
    look_h = math.sqrt(lx * lx + lz * lz)
    move_h = math.hypot(v[0], v[2])
    lift = math.cos(lean) ** 2
    vx, vy, vz = v[0], v[1] + gravity * (-1.0 + lift * 0.75), v[2]
    if vy < 0.0 and look_h > 0.0:
        c = vy * -0.1 * lift
        vx += lx * c / look_h
        vy += c
        vz += lz * c / look_h
    if lean < 0.0 and look_h > 0.0:
        c = move_h * -mth_sin(lean) * 0.04
        vx += -lx * c / look_h
        vy += c * 3.2
        vz += -lz * c / look_h
    if look_h > 0.0:
        vx += (lx / look_h * move_h - vx) * 0.1
        vz += (lz / look_h * move_h - vz) * 0.1
    return vx * float(f32(0.99)), vy * float(f32(0.98)), vz * float(f32(0.99))


def unwrap_yaw(wrapped: list[float], start: float) -> list[float]:
    """Undo Flashback's Mth.wrapDegrees on a per-tick yaw sequence.

    The client's yaw is unbounded and physics sees its exact float bits. A
    wrapped float plus the right multiple of 360 is the original exactly when
    both sit in binades where the sum is representable, which holds for any
    yaw a player actually reaches; the extractor's physics check catches it
    if not. k is chosen for continuity (a player can't turn 180 in one tick).
    """
    out, prev = [], start
    for w in wrapped:
        k = round((prev - w) / 360.0)
        y = float(f32(f32(w) + f32(360.0 * k)))
        out.append(y)
        prev = y
    return out
