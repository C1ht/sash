# SPDX-License-Identifier: Apache-2.0
#
# The pinned similarity setup (owner, after M13b): the reference sources (Smithay v0.7.0's
# smallvil and anvil, by commit and by a hash of their files) and a deterministic
# copydetect. Imported by summary.py and pair.py before copydetect is used.
#
# copydetect 0.5.0 fingerprints with Python's built-in hash(), which is randomized per
# process, so unpinned runs pick different fingerprints and the same files score
# differently from run to run. stable_hash replaces it inside copydetect.
import hashlib, importlib.metadata, pathlib, sys

SMITHAY_TAG = "v0.7.0"
SMITHAY_COMMIT = "a166cf4c94b5aedc332a65aa1dd753e8148829c3"
# sha256 over "path\0sha256(content)\n" for every file under smallvil/src and
# anvil/src, sorted by path (see tree_hash).
SOURCES_SHA256 = "787f07fc78ae65e8dd3776d841f812387901a75fca20f9a67191cfef9ecfcfe0"
COPYDETECT = "0.5.0"
PYGMENTS = "2.21.0"


def tree_hash(ref_root):
    root = pathlib.Path(ref_root)
    files = sorted(
        p for d in ("smallvil/src", "anvil/src") for p in (root / d).rglob("*") if p.is_file()
    )
    h = hashlib.sha256()
    for p in files:
        rel = p.relative_to(root).as_posix()
        h.update(rel.encode() + b"\0" + hashlib.sha256(p.read_bytes()).hexdigest().encode() + b"\n")
    return h.hexdigest(), len(files)


def stable_hash(text):
    return int.from_bytes(hashlib.blake2b(text.encode(), digest_size=8).digest(), "big", signed=True)


def setup(ref_root=None):
    """Checks the tool versions (and the reference, if given), makes copydetect
    deterministic, and returns the header line to print."""
    versions = {
        "copydetect": importlib.metadata.version("copydetect"),
        "pygments": importlib.metadata.version("pygments"),
    }
    if versions != {"copydetect": COPYDETECT, "pygments": PYGMENTS}:
        sys.exit(f"similarity: tool versions {versions}, pinned copydetect {COPYDETECT}, pygments {PYGMENTS}")
    import copydetect.utils
    copydetect.utils.hash = stable_hash
    line = f"tools: copydetect {COPYDETECT}, pygments {PYGMENTS}, deterministic fingerprints (blake2b)"
    if ref_root is not None:
        digest, count = tree_hash(ref_root)
        if digest != SOURCES_SHA256:
            sys.exit(f"similarity: reference sources {digest} ({count} files) are not the pinned {SOURCES_SHA256}")
        line += f"\nreference: Smithay {SMITHAY_TAG} {SMITHAY_COMMIT}, smallvil+anvil sources sha256 {digest} ({count} files)"
    return line
