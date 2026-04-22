"""
Download a slice of Henry Zhu's decoded-replay-packets Hugging Face
dataset into the gitignored local cache.

Defaults to one batch file (~80 MB) from patch 12.22. Full dataset is
~108 GB across 1,348 batch files; do not try to download it all unless
you have the disk and bandwidth.

Usage:
    python scripts/zhu_dataset_fetch.py                   # 1 batch of 12_22
    python scripts/zhu_dataset_fetch.py --patch 12_23 --n 3
    python scripts/zhu_dataset_fetch.py --schema-only

Cache lives at reference/zhu-dataset/ (gitignored).
"""

import argparse
import os
import pathlib
import sys

try:
    from huggingface_hub import hf_hub_download
except ImportError:
    print("Install: pip install huggingface_hub", file=sys.stderr)
    sys.exit(1)

REPO = "maknee/league-of-legends-decoded-replay-packets"
REPO_TYPE = "dataset"
REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
CACHE = REPO_ROOT / "reference" / "zhu-dataset"


def fetch(filename: str) -> str:
    path = hf_hub_download(
        repo_id=REPO,
        filename=filename,
        repo_type=REPO_TYPE,
        local_dir=str(CACHE),
    )
    return path


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--patch", default="12_22", help="patch directory, e.g. 12_22")
    ap.add_argument(
        "--n", type=int, default=1, help="number of consecutive batches to fetch"
    )
    ap.add_argument(
        "--schema-only",
        action="store_true",
        help="only download packets.py",
    )
    args = ap.parse_args()

    CACHE.mkdir(parents=True, exist_ok=True)

    schema = fetch("packets.py")
    print(f"schema : {schema}")

    if args.schema_only:
        return

    for i in range(1, args.n + 1):
        rel = f"{args.patch}/batch_{i:03d}.jsonl.gz"
        p = fetch(rel)
        size_mb = os.path.getsize(p) / 1024 / 1024
        print(f"batch  : {p}  ({size_mb:.1f} MB)")


if __name__ == "__main__":
    main()
