#!/bin/sh
# Regenerate data/blocks-26.2.tsv.gz (block states: collision boxes, offsets, movement
# classes; see world.py's docstring) from the
# MC 26.2 jar that Fabric Loom caches (elytra-vario builds put it there).
# usage: tools/block_dump.sh
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
jar="$HOME/.gradle/caches/fabric-loom/26.2/minecraft-merged.jar"
meta="$HOME/Library/Application Support/ModrinthApp/meta"
cp=$(python3 - "$meta" <<'PY'
import json, sys, os
meta = sys.argv[1]
j = json.load(open(f"{meta}/versions/26.2-0.19.5/26.2-0.19.5.json"))
paths = []
for lib in j["libraries"]:
    g, a, v, *cls = lib["name"].split(":")
    if cls:
        continue  # natives
    p = f"{meta}/libraries/{g.replace('.', '/')}/{a}/{v}/{a}-{v}.jar"
    if os.path.exists(p):
        paths.append(p)
print(":".join(paths))
PY
)
exec java -cp "$jar:$cp" "$here/tools/BlockDump.java" "$here/data/blocks-26.2.tsv.gz"
