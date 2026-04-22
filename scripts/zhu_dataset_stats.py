"""
Profile a slice of Henry Zhu's decoded-replay dataset: packet-class
frequencies, field unions, value-range observations. Writes a summary
to stdout; refuse to persist anything that includes PUUIDs or player
names (defensive PII check).

Usage:
    python scripts/zhu_dataset_stats.py                      # defaults
    python scripts/zhu_dataset_stats.py --games 50
    python scripts/zhu_dataset_stats.py --file reference/zhu-dataset/12_22/batch_001.jsonl.gz
"""

import argparse
import collections
import gzip
import json
import pathlib
import sys

REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_FILE = REPO_ROOT / "reference" / "zhu-dataset" / "12_22" / "batch_001.jsonl.gz"

# Keys that look like they could contain PII. We don't print values for
# these; only record their presence.
PII_KEY_MARKERS = ("puuid", "riot_id", "summoner_name", "display_name")


def is_pii_key(key: str) -> bool:
    lk = key.lower()
    return any(m in lk for m in PII_KEY_MARKERS)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--file", default=str(DEFAULT_FILE))
    ap.add_argument("--games", type=int, default=20)
    args = ap.parse_args()

    path = pathlib.Path(args.file)
    if not path.exists():
        print(f"missing: {path}. Run scripts/zhu_dataset_fetch.py first.", file=sys.stderr)
        sys.exit(1)

    games = 0
    total_events = 0
    class_counts: collections.Counter = collections.Counter()
    class_fields: dict[str, set] = {}
    class_sample: dict[str, dict] = {}

    with gzip.open(path, "rt", encoding="utf-8") as f:
        for line in f:
            match = json.loads(line)
            events = match.get("events", [])
            games += 1
            total_events += len(events)
            for ev in events:
                for cname, payload in ev.items():
                    class_counts[cname] += 1
                    if isinstance(payload, dict):
                        fields = class_fields.setdefault(cname, set())
                        fields.update(payload.keys())
                        if cname not in class_sample:
                            class_sample[cname] = payload
            if games >= args.games:
                break

    print(f"file         : {path}")
    print(f"games read   : {games}")
    print(f"total events : {total_events}")
    print(f"avg / game   : {total_events / max(1, games):.0f}")
    print(f"classes seen : {len(class_counts)}")
    print()
    print("class frequencies:")
    print(f"  {'count':>10}  {'share':>6}  class")
    for cname, n in class_counts.most_common():
        share = n / total_events * 100
        print(f"  {n:>10}  {share:>5.1f}%  {cname}")

    print()
    print("fields per class:")
    for cname in sorted(class_fields):
        fields = sorted(class_fields[cname])
        any_pii = [k for k in fields if is_pii_key(k)]
        note = f"  [PII-possible keys: {any_pii}]" if any_pii else ""
        print(f"  {cname}: {fields}{note}")

    print()
    print("Reminder: do not commit the .jsonl.gz file or any decoded-JSON output that")
    print("contains PUUIDs / player names. Stats on this page are safe to commit.")


if __name__ == "__main__":
    main()
