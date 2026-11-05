# SPDX-License-Identifier: Apache-2.0
#
# Summary of copydetect results: per sash-wm file, max similarity vs smallvil and anvil,
# and each distinct sash-wm block of >= MIN_LINES lines with the references it matches.
# Usage: python -I tools/similarity/summary.py SASH_SRC REF_ROOT NOISE GUARANTEE MIN_LINES
import sys, pathlib, collections
sys.path.insert(0, str(pathlib.Path(__file__).parent))
import pinned
header = pinned.setup(sys.argv[2])
from copydetect import CodeFingerprint, compare_files

sash, ref = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
k, guarantee, min_lines = int(sys.argv[3]), int(sys.argv[4]), int(sys.argv[5])
win = guarantee - k + 1
tests = sorted(sash.rglob("*.rs"))
refs = sorted((ref / "smallvil/src").rglob("*.rs")) + sorted((ref / "anvil/src").rglob("*.rs"))
fp = {f: CodeFingerprint(str(f), k, win) for f in tests + refs}
line = lambda code, i: code.count("\n", 0, i) + 1

print(header)
print(f"method: winnowing, pygments Rust lexer, identifiers normalized; noise_t={k} guarantee_t={guarantee}")
print(f"{'sash-wm file':28} {'max vs smallvil':>16} {'max vs anvil':>13}")
blocks = collections.defaultdict(set)
for t in tests:
    best = {"smallvil": (0, ""), "anvil": (0, "")}
    for r in refs:
        overlap, (sim_t, _), (sl_t, sl_r) = compare_files(fp[t], fp[r])
        if overlap == 0:
            continue
        group = r.relative_to(ref).parts[0]
        if sim_t > best[group][0]:
            best[group] = (sim_t, r.name)
        for (a, b), (c, d) in zip(zip(*sl_t), zip(*sl_r)):
            l0, l1 = line(fp[t].raw_code, a), line(fp[t].raw_code, b)
            if l1 - l0 + 1 >= min_lines:
                r0, r1 = line(fp[r].raw_code, c), line(fp[r].raw_code, d)
                blocks[(str(t.relative_to(sash)), l0, l1)].add(f"{r.relative_to(ref)}:{r0}-{r1}")
    fmt = lambda g: f"{best[g][0]:.1%} {best[g][1]}" if best[g][0] else "-"
    print(f"{str(t.relative_to(sash)):28} {fmt('smallvil'):>16} {fmt('anvil'):>13}")
print(f"\nblocks of >= {min_lines} lines:")
for (f, l0, l1), rs in sorted(blocks.items()):
    print(f"  {f}:{l0}-{l1} ({l1-l0+1} lines)  <- {', '.join(sorted(rs))}")
