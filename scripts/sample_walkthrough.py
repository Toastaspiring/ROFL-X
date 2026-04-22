"""
Phase 1 step 3: produce raw data for docs/SAMPLE_A_WALKTHROUGH.md.

Reads one real .rofl file, does NOT decrypt any packet contents, only:
  - dumps the file header
  - extracts the metadata JSON trailer (with names redacted)
  - locates and dumps the first chunk header
  - attempts zstd decompression of the first chunk
  - prints post-decompression block framing bytes

This script intentionally does not port any Mowokuma code, it inspects bytes
and prints them. It exists only to populate the walkthrough doc.

Not committed to the repo as code; lives in scripts/ for reproducibility.
"""

import json
import os
import re
import struct
import sys
import hashlib

try:
    import zstandard as zstd
except ImportError:
    zstd = None

SAMPLE = r"C:\Users\louis\Documents\League of Legends\replays\EUW1-7828362936.rofl"


def hex_dump(data: bytes, start: int = 0, length: int = 64, prefix: str = "  ") -> str:
    lines = []
    for i in range(0, min(length, len(data)), 16):
        chunk = data[i : i + 16]
        hex_part = " ".join(f"{b:02x}" for b in chunk)
        ascii_part = "".join(chr(b) if 32 <= b < 127 else "." for b in chunk)
        lines.append(f"{prefix}{start + i:08x}  {hex_part:<47}  |{ascii_part}|")
    return "\n".join(lines)


def main() -> None:
    with open(SAMPLE, "rb") as f:
        data = f.read()

    total = len(data)
    sha256 = hashlib.sha256(data).hexdigest()[:16]

    print("=" * 72)
    print(f"SAMPLE FILE (not committed): {os.path.basename(SAMPLE)}")
    print(f"Size: {total} bytes ({total/1024/1024:.2f} MiB)")
    print(f"SHA-256 prefix (for referencing): {sha256}")
    print("=" * 72)

    # --- 1. File-start hex dump ---
    print("\n## 1. File header (first 96 bytes)\n")
    print(hex_dump(data, 0, 96))

    # Per Mowokuma: version is ASCII at bytes [16..20].
    version_bytes = data[16:24]
    print(f"\nbytes [0..6]   = {data[0:6]!r}  (likely 'RIOT' magic?)")
    print(f"bytes [16..24] = {version_bytes!r}  (Mowokuma reads version from [16..20])")

    # Mowokuma's skip logic:
    #   drain 0x10 bytes
    #   if buffer[0xC] == 1: drain 0xC more  => total skip = 0x10 + 0xC = 0x1C
    #   else: drain 0xD more                 => total skip = 0x10 + 0xD = 0x1D
    discriminator = data[0x10 + 0xC]
    header_skip = 0x1C if discriminator == 1 else 0x1D
    print(f"\nbyte  [0x1C]   = 0x{discriminator:02x}  (discriminator: 1 => header=0x1C, else 0x1D)")
    print(f"=> Header size by Mowokuma's rule: 0x{header_skip:x} ({header_skip}) bytes")
    print("\nHeader region (0..header_skip):")
    print(hex_dump(data[:header_skip], 0, header_skip))

    # --- 2. File-end trailer (signature + metadata + size footer) ---
    meta_size = struct.unpack_from("<I", data, total - 4)[0]
    print(f"\n## 2. File tail\n")
    print(f"u32 LE at [len-4] = {meta_size} bytes of metadata JSON")

    meta_start = total - 4 - meta_size
    sig_start = meta_start - 0x100
    meta_json_bytes = data[meta_start:total - 4]

    print(f"metadata JSON bytes at [{meta_start}..{total - 4}] ({meta_size} bytes)")
    print(f"signature bytes at [{sig_start}..{meta_start}] (256 bytes)")
    print("\nFirst 64 bytes of signature:")
    print(hex_dump(data[sig_start:sig_start + 64], sig_start, 64))

    # Parse metadata JSON
    try:
        meta_text = meta_json_bytes.decode("utf-8")
        meta = json.loads(meta_text)
    except Exception as e:
        print(f"\nFAILED to decode metadata JSON: {e}")
        return

    # Redact PII from meta before printing a preview.
    redacted = {}
    for k, v in meta.items():
        if k == "statsJson":
            continue  # handle separately
        redacted[k] = v

    print("\nMetadata keys (top level):")
    for k in meta.keys():
        print(f"  - {k}: {type(meta[k]).__name__}")

    print(f"\n  gameLength = {meta.get('gameLength')!r}")
    print(f"  gameVersion = {meta.get('gameVersion')!r}")
    print(f"  lastGameChunkId = {meta.get('lastGameChunkId')!r}")
    print(f"  lastKeyFrameId = {meta.get('lastKeyFrameId')!r}")

    stats_raw = meta.get("statsJson")
    if stats_raw:
        try:
            stats = json.loads(stats_raw)
            print(f"\n  statsJson: array of {len(stats)} player entries")
            if stats:
                one = stats[0]
                print(f"  statsJson[0] has {len(one)} total keys")
                # Check for the fields Mowokuma relies on.
                for key in ("NAME", "SKIN", "TEAM", "WIN", "RIOT_ID_GAME_NAME",
                             "PUUID", "SUMMONER_NAME"):
                    val = one.get(key)
                    if val is None:
                        print(f"    {key!r}: <missing>")
                    elif key in ("NAME", "RIOT_ID_GAME_NAME", "PUUID", "SUMMONER_NAME"):
                        print(f"    {key!r}: <redacted, len={len(str(val))}>")
                    else:
                        print(f"    {key!r}: {val!r}")
                # Sample other non-stat keys.
                non_stat_keys = [
                    k for k in sorted(one.keys())
                    if not k.startswith("2026_") and not k.startswith("2025_")
                    and not k.isupper() or k in ("NAME","SKIN","TEAM","WIN")
                ]
                print(f"  non-stat non-uppercase keys (first 20): {non_stat_keys[:20]}")
        except Exception as e:
            print(f"  failed to parse statsJson: {e}")

    # --- 3. First chunk header ---
    # Chunk area starts immediately after header_skip, ends at sig_start.
    chunks_start = header_skip
    print(f"\n## 3. First chunk\n")
    print(f"Chunk region: [{chunks_start}..{sig_start}] ({sig_start - chunks_start} bytes)")

    # Per Mowokuma: chunk header is 0x11 (17) bytes:
    #   u32 chunk_id, u8 chunk_type, u32 chunk_id_2, u32 uncompressed_len, u32 compressed_len
    (chunk_id, chunk_type, chunk_id_2, u_len, c_len) = struct.unpack_from(
        "<IBIII", data, chunks_start
    )
    print(f"\nFirst chunk header at offset 0x{chunks_start:x} ({chunks_start}):")
    print(hex_dump(data[chunks_start:chunks_start + 17], chunks_start, 17))
    print(f"\n  chunk_id            = {chunk_id}")
    print(f"  chunk_type          = 0x{chunk_type:02x}")
    print(f"  chunk_id_2          = {chunk_id_2}")
    print(f"  uncompressed_len    = {u_len}")
    print(f"  compressed_len      = {c_len}")

    payload_start = chunks_start + 17
    print(f"\nFirst 64 bytes of compressed chunk payload:")
    print(hex_dump(data[payload_start:payload_start + 64], payload_start, 64))

    # zstd magic = 28 b5 2f fd
    magic = data[payload_start:payload_start + 4]
    zstd_magic = bytes.fromhex("28b52ffd")
    print(f"\nPayload first 4 bytes = {magic.hex()}")
    print(f"zstd magic bytes are   = 28b52ffd  (matches? {magic == zstd_magic})")

    # --- 4. Decompress first chunk if zstd is installed ---
    if zstd is not None and c_len > 0:
        payload = data[payload_start:payload_start + c_len]
        try:
            dctx = zstd.ZstdDecompressor()
            decompressed = dctx.decompress(payload, max_output_size=u_len * 2)
            print(f"\n## 4. Decompressed first chunk\n")
            print(f"Decompressed: {len(decompressed)} bytes (expected {u_len}, match={len(decompressed)==u_len})")
            print("\nFirst 128 bytes of decompressed block stream:")
            print(hex_dump(decompressed, 0, 128))

            # Try to parse the first block by Mowokuma's rules.
            print("\n### Parsing the first block per Mowokuma's marker-byte rules\n")
            it = iter(decompressed)
            marker = next(it)
            print(f"marker byte = 0x{marker:02x}  "
                  f"(bits: 0x80={'set' if marker & 0x80 else 'clear'} "
                  f"0x40={'set' if marker & 0x40 else 'clear'} "
                  f"0x20={'set' if marker & 0x20 else 'clear'} "
                  f"0x10={'set' if marker & 0x10 else 'clear'})")

            acc = 0.0
            if marker & 0x80:
                d = next(it)
                acc += d * 0.001
                print(f"  timestamp: relative u8 delta = {d} => accumulated = {acc:.3f} s")
            else:
                ts_bytes = bytes(next(it) for _ in range(4))
                acc = struct.unpack("<f", ts_bytes)[0]
                print(f"  timestamp: absolute f32 = {acc}")

            if marker & 0x10:
                length = next(it)
                print(f"  length (u8): {length}")
            else:
                lb = bytes(next(it) for _ in range(4))
                length = struct.unpack("<I", lb)[0]
                print(f"  length (u32): {length}")

            if marker & 0x40:
                print(f"  packet_id: reused from previous (none on first block => 0)")
                packet_id = 0
            else:
                pb = bytes(next(it) for _ in range(2))
                packet_id = struct.unpack("<H", pb)[0]
                print(f"  packet_id (u16): {packet_id} (0x{packet_id:04x})")

            if marker & 0x20:
                d = next(it)
                print(f"  param: relative u8 delta = {d} => absolute = {d} (prev was 0)")
            else:
                pb = bytes(next(it) for _ in range(4))
                param = struct.unpack("<I", pb)[0]
                print(f"  param (u32): {param} (0x{param:08x})")

            body = bytes(next(it) for _ in range(min(length, 32)))
            print(f"  payload (first {len(body)} bytes): {body.hex()}")

            # --- 5. Block opcode histogram over the first chunk ---
            print("\n### Opcode histogram, first decompressed chunk\n")
            it2 = iter(decompressed)
            prev_pid = 0
            prev_param = 0
            acc = 0.0
            counts: dict[int, int] = {}
            errors = 0
            try:
                while True:
                    m = next(it2)
                    if m & 0x80:
                        acc += next(it2) * 0.001
                    else:
                        ts_bytes = bytes(next(it2) for _ in range(4))
                        acc = struct.unpack("<f", ts_bytes)[0]
                    if m & 0x10:
                        length = next(it2)
                    else:
                        lb = bytes(next(it2) for _ in range(4))
                        length = struct.unpack("<I", lb)[0]
                    if m & 0x40:
                        pid = prev_pid
                    else:
                        pb = bytes(next(it2) for _ in range(2))
                        pid = struct.unpack("<H", pb)[0]
                    if m & 0x20:
                        prev_param += next(it2)
                    else:
                        pb = bytes(next(it2) for _ in range(4))
                        prev_param = struct.unpack("<I", pb)[0]
                    # consume payload
                    for _ in range(length):
                        next(it2)
                    prev_pid = pid
                    counts[pid] = counts.get(pid, 0) + 1
            except StopIteration:
                pass
            except Exception as e:
                errors += 1

            total_blocks = sum(counts.values())
            print(f"Total blocks parsed in keyframe chunk: {total_blocks} (errors: {errors})")
            print(f"Distinct packet_ids: {len(counts)}")
            if counts:
                print(f"\nAll packet_ids in this (keyframe) chunk:")
                for pid, cnt in sorted(counts.items(), key=lambda x: -x[1]):
                    print(f"  0x{pid:04x} ({pid:>5d})  x{cnt}")
        except Exception as e:
            print(f"\nzstd decompression FAILED: {e}")

        # --- 5. Walk all chunks, find first non-keyframe ---
        print("\n## 5. Walking all chunks\n")
        dctx = zstd.ZstdDecompressor()
        cursor = chunks_start
        chunks_summary = []  # (idx, offset, id, type, id2, u_len, c_len)
        delta_data = None
        delta_idx = None
        idx = 0
        while cursor + 17 <= sig_start:
            (cid, ctype, cid2, culen, cclen) = struct.unpack_from("<IBIII", data, cursor)
            # Sanity check, if values are crazy, stop (we may be misaligned / past the chunk area).
            if cclen > 10 * 1024 * 1024 or culen > 50 * 1024 * 1024:
                print(f"  chunk {idx} at 0x{cursor:x} has implausible sizes (ulen={culen}, clen={cclen}), stopping")
                break
            body_len = cclen if cclen > 0 else culen
            if cursor + 17 + body_len > sig_start:
                print(f"  chunk {idx} at 0x{cursor:x} would overrun signature boundary, stopping")
                break
            chunks_summary.append((idx, cursor, cid, ctype, cid2, culen, cclen))
            if cclen > 0:
                payload = data[cursor + 17:cursor + 17 + cclen]
                if ctype != 0x02 and delta_data is None:
                    try:
                        delta_data = dctx.decompress(payload, max_output_size=culen * 2)
                        delta_idx = idx
                    except Exception as e:
                        print(f"    chunk {idx}: decompress failed: {e}")
            cursor += 17 + body_len
            idx += 1

        print(f"Total chunks walked: {len(chunks_summary)}")
        type_counts: dict[int, int] = {}
        id2_counts: dict[int, int] = {}
        for (_, _, _, t, cid2, _, _) in chunks_summary:
            type_counts[t] = type_counts.get(t, 0) + 1
            id2_counts[cid2] = id2_counts.get(cid2, 0) + 1
        print(f"Chunk-type distribution: {dict(sorted(type_counts.items()))}")
        print(f"Distinct chunk_id_2 values: {len(id2_counts)}; top 5: "
              f"{sorted(id2_counts.items(), key=lambda x: -x[1])[:5]}")
        print(f"\nFirst 15 chunks:")
        print(f"  {'idx':>4} {'offset':>10} {'id':>4} {'type':>4} {'id2':>12} {'ulen':>8} {'clen':>8}")
        for (i, off, cid, ctype, cid2, culen, cclen) in chunks_summary[:15]:
            print(f"  {i:>4} 0x{off:08x} {cid:>4} 0x{ctype:02x} 0x{cid2:10x} {culen:>8} {cclen:>8}")
        print(f"\nLast 5 chunks:")
        for (i, off, cid, ctype, cid2, culen, cclen) in chunks_summary[-5:]:
            print(f"  {i:>4} 0x{off:08x} {cid:>4} 0x{ctype:02x} 0x{cid2:10x} {culen:>8} {cclen:>8}")

        if delta_data is not None:
            print(f"\n### First non-keyframe chunk (idx={delta_idx})\n")
            print(f"Decompressed size: {len(delta_data)} bytes")
            print("\nFirst 128 bytes of decompressed block stream:")
            print(hex_dump(delta_data, 0, 128))

            print("\n### Opcode histogram, first non-keyframe chunk\n")
            it2 = iter(delta_data)
            prev_pid = 0
            prev_param = 0
            acc = 0.0
            counts2: dict[int, int] = {}
            errors2 = 0
            try:
                while True:
                    m = next(it2)
                    if m & 0x80:
                        acc += next(it2) * 0.001
                    else:
                        ts_bytes = bytes(next(it2) for _ in range(4))
                        acc = struct.unpack("<f", ts_bytes)[0]
                    if m & 0x10:
                        length = next(it2)
                    else:
                        lb = bytes(next(it2) for _ in range(4))
                        length = struct.unpack("<I", lb)[0]
                    if m & 0x40:
                        pid = prev_pid
                    else:
                        pb = bytes(next(it2) for _ in range(2))
                        pid = struct.unpack("<H", pb)[0]
                    if m & 0x20:
                        prev_param += next(it2)
                    else:
                        pb = bytes(next(it2) for _ in range(4))
                        prev_param = struct.unpack("<I", pb)[0]
                    for _ in range(length):
                        next(it2)
                    prev_pid = pid
                    counts2[pid] = counts2.get(pid, 0) + 1
            except StopIteration:
                pass
            except Exception:
                errors2 += 1

            print(f"Total blocks parsed: {sum(counts2.values())} (errors: {errors2})")
            print(f"Distinct packet_ids: {len(counts2)}")
            print(f"\nTop 30 packet_ids by frequency:")
            for pid, cnt in sorted(counts2.items(), key=lambda x: -x[1])[:30]:
                print(f"  0x{pid:04x} ({pid:>5d})  x{cnt}")

        # --- 6. Global opcode coverage (all non-keyframe chunks) ---
        print("\n## 6. Global opcode histogram (all non-keyframe chunks)\n")
        global_counts: dict[int, int] = {}
        global_errors = 0
        for (idx, off, cid, ctype, cid2, culen, cclen) in chunks_summary:
            if ctype == 0x02 or cclen == 0:
                continue
            payload = data[off + 17:off + 17 + cclen]
            try:
                dec = dctx.decompress(payload, max_output_size=culen * 2)
            except Exception:
                global_errors += 1
                continue
            it3 = iter(dec)
            prev_pid = 0
            prev_param = 0
            try:
                while True:
                    m = next(it3)
                    if m & 0x80:
                        next(it3)
                    else:
                        for _ in range(4):
                            next(it3)
                    if m & 0x10:
                        length = next(it3)
                    else:
                        lb = bytes(next(it3) for _ in range(4))
                        length = struct.unpack("<I", lb)[0]
                    if m & 0x40:
                        pid = prev_pid
                    else:
                        pb = bytes(next(it3) for _ in range(2))
                        pid = struct.unpack("<H", pb)[0]
                    if m & 0x20:
                        prev_param += next(it3)
                    else:
                        for _ in range(4):
                            next(it3)
                    for _ in range(length):
                        next(it3)
                    prev_pid = pid
                    global_counts[pid] = global_counts.get(pid, 0) + 1
            except StopIteration:
                pass
            except Exception:
                global_errors += 1

        print(f"Total distinct packet_ids seen: {len(global_counts)}")
        print(f"Total blocks seen: {sum(global_counts.values())}")
        print(f"Chunk decode errors: {global_errors}")
        print(f"\nTop 40 opcodes across whole file:")
        for pid, cnt in sorted(global_counts.items(), key=lambda x: -x[1])[:40]:
            print(f"  0x{pid:04x} ({pid:>5d})  x{cnt}")
        print(f"\nAll opcodes (sorted by id):")
        for pid in sorted(global_counts):
            print(f"  0x{pid:04x} ({pid:>5d})  x{global_counts[pid]}")
    else:
        if zstd is None:
            print("\n[zstandard module not available, skipping decompression]")


if __name__ == "__main__":
    main()
