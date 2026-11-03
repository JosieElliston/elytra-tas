"""Writes the hand-made world that tests/roundtrip.rs checks, with the repo's
extract_world.write_world: python3 make_small_world.py OUT.world.gz

Columns (chunk x, z) (-1, 0), (0, 0), (1, 0) are loaded; the bounds also cover
z chunk 1, which is written as explicit unknown sections. Build height -16..31.
The block tags are EMC-like: no can_glide_through, scaffolding not climbable.
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[4]))
import extract_world  # noqa: E402
import world  # noqa: E402


def default(name: str) -> str:
    return next(b.state for b in world.blocks() if b.name == name and b.default)


w = world.World(min_y=-16, height=48)
w.report.dimension = "test:small"
for cx in (-1, 0, 1):
    w.add_column(cx, 0, fill=world.state_ids()["minecraft:air"])
for x in range(-16, 32):
    for z in range(16):
        w.set(x, 0, z, "minecraft:stone")
w.set(3, 1, 3, "minecraft:oak_slab[type=bottom,waterlogged=false]")
w.set(20, 1, 5, default("minecraft:bamboo"))
w.set(-5, 1, 2, default("minecraft:vine"))
w.set(-6, 1, 2, default("minecraft:ladder"))
w.set(-7, 1, 2, default("minecraft:scaffolding"))
w.set(9, 31, 9, "minecraft:glass")
ids = {b.name: b.block_id for b in world.blocks()}
w.report.block_tags = {"minecraft:climbable": [ids["minecraft:ladder"], ids["minecraft:vine"]]}
extract_world.write_world(sys.argv[1], w, "hand-made (make_small_world.py)", None,
                          [[-16, -16, 0], [31, 47, 31]], {"note": "test"})
