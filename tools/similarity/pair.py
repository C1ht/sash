# SPDX-License-Identifier: Apache-2.0
#
# Shows the filtered tokens copydetect matched between two files (side by side check).
# Usage: python -I tools/similarity/pair.py SASH_FILE REF_FILE NOISE GUARANTEE
import sys
from copydetect import CodeFingerprint, compare_files
from copydetect.utils import get_copied_slices, find_fingerprint_overlap
a, b, k, g = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
fa, fb = CodeFingerprint(a, k, g - k + 1), CodeFingerprint(b, k, g - k + 1)
ia, ib = find_fingerprint_overlap(fa.hashes, fb.hashes, fa.hash_idx, fb.hash_idx)
sa = get_copied_slices(ia, k)
print("filtered-token matches in", a.split("/")[-1])
for s, e in zip(*sa):
    print(f"  [{s}:{e}] {fa.filtered_code[s:e]!r}")
