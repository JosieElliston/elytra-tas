#!/usr/bin/env python3
"""States a start could hand over from, for the `routes` bin's `start` phase: each local attempt's last
deploy before the exit and the top of the flight after it, labeled by ground class (`run_classes.py`),
with the path it took to the exit. Remote players' poses are too rough to start from.

usage: python3 tools/start_states.py CLASSES.json OUT.json SEGMENTS.jsonl.gz...
"""
import collections
import gzip
import json
import sys

PER_KIND = 6
# path past the exit, so a seed has somewhere to aim
PAST_EXIT = 12


def main():
    classes = {r["id"]: r for r in json.load(open(sys.argv[1]))}
    out = collections.defaultdict(list)
    for path in sys.argv[3:]:
        for line in gzip.open(path, "rt"):
            a = json.loads(line)
            r = classes.get(a["id"])
            e = a["exit"]
            if r is None or e is None or not a["local"] or e < 3:
                continue
            xyz, gl, ground = a["xyz"], a["glide"], a["ground"]
            deploys = [i for i in range(1, e) if gl[i] and not gl[i - 1] and not ground[i]]
            if not deploys:
                continue
            i = deploys[-1]
            # glides to the exit with nothing odd
            if not all(gl[j] and not ground[j] for j in range(i, e)):
                continue
            peak = max(range(i, e), key=lambda j: xyz[j][1])
            g = a["gates"][2] if a["gates"] else None
            for kind, k in (("deploy", i), ("peak", peak)):
                if kind == "peak" and peak == i:
                    continue
                v = [xyz[k][m] - xyz[k - 1][m] for m in range(3)]
                out[(r["class"], kind)].append(
                    {
                        "label": f'{r["class"]}|{kind}|{a["player"]}:{a["id"].split(":")[-1]}',
                        "class": r["class"],
                        "kind": kind,
                        "x": xyz[k] + v,
                        "path": xyz[k : e + PAST_EXIT],
                        "gate": g,
                        "touches": r["touches"],
                    }
                )
    keep = []
    for key, v in out.items():
        # by how it did, and by how high: height is the question
        v.sort(key=lambda c: (c["gate"] is None, c["gate"] or 0))
        by_height = sorted(v, key=lambda c: -c["x"][1])
        chosen = {c["label"]: c for c in v[:PER_KIND] + by_height[: PER_KIND // 2]}
        keep.extend(chosen.values())
    json.dump(keep, open(sys.argv[2], "w"))
    print(len(keep), "states,", len(out), "class/kinds")


main()
