//! Byte-pattern matcher for porting decoder RVAs across patches.
//!
//! Given a JSON dump of known decoder functions from a donor patch (produced
//! by `scripts/ghidra/export_decoders.py`) and a target patch's `text.bin`,
//! find candidate RVA ranges in the target that match each donor.
//!
//! The matching strategy is anchor-clustering. From each donor function we
//! extract several short distinctive byte patterns at known offsets within
//! the function. We search the target text for each anchor independently;
//! a candidate function start is a target offset where many anchors hit
//! at the *expected relative offset*. The best-scored cluster wins.
//!
//! This is not a full diff: it does not detect renamed registers or
//! re-ordered basic blocks. It catches the common case where Riot rebuilds
//! the binary but the decoder body is byte-near-identical, just relocated.
//! For decoders that were genuinely rewritten between patches, expect a low
//! score and fall back to manual disassembly.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Result, RoflError};

/// One donor function as exported by the Ghidra script.
#[derive(Debug, Clone, Deserialize)]
pub struct DonorFunction {
    /// Symbolic name (e.g. `"ward_spawn_decrypt"`).
    pub name: String,
    /// Function start RVA in the donor binary, hex string `"0xe3d7b0"`.
    pub rva_start: String,
    /// Function end RVA (exclusive) in the donor binary, hex string.
    pub rva_end: String,
    /// Distinctive byte sequences from this function, paired with the
    /// offset within the function where they appear. Picked by the Ghidra
    /// script to avoid known-volatile bytes (immediates, RIP-relative
    /// displacements).
    pub anchors: Vec<Anchor>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Anchor {
    /// Byte offset within the donor function.
    pub offset_in_function: u64,
    /// The bytes themselves, encoded as a hex string with no separators
    /// (e.g. `"4889c34c8b0e"`).
    pub bytes_hex: String,
}

/// Top-level shape of the donor JSON file.
#[derive(Debug, Clone, Deserialize)]
pub struct DonorDump {
    /// Free-form patch identifier from the donor binary, e.g. `"15.5"`.
    pub patch: String,
    /// Hex string for the donor's `.text` section RVA. Used to translate
    /// function RVAs into offsets within the donor's `text.bin`.
    pub text_rva: String,
    pub functions: Vec<DonorFunction>,
}

/// One match candidate inside the target text.
#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    /// Suggested function-start RVA in the target, hex string.
    pub rva_start_hex: String,
    /// Suggested function-end RVA in the target (donor length applied).
    pub rva_end_hex: String,
    /// Number of anchor matches that landed at the expected relative offset.
    pub matched_anchors: usize,
    /// Total anchor count in the donor.
    pub total_anchors: usize,
    /// `matched_anchors / total_anchors`, in [0.0, 1.0].
    pub score: f32,
}

/// Match report for one donor function.
#[derive(Debug, Clone, Serialize)]
pub struct ScanReport {
    pub donor_name: String,
    pub donor_rva_start: String,
    pub donor_rva_end: String,
    pub anchor_count: usize,
    pub top_candidates: Vec<Candidate>,
}

/// Scan one target `text.bin` for candidate matches of every donor function.
///
/// `target_text_rva` is the RVA at which `target_text` is mapped (so we can
/// translate target offsets back into RVAs in the report).
pub fn scan_all(
    donors: &DonorDump,
    target_text: &[u8],
    target_text_rva: u64,
    top_k: usize,
) -> Result<Vec<ScanReport>> {
    let mut reports = Vec::with_capacity(donors.functions.len());
    for f in &donors.functions {
        reports.push(scan_one(f, target_text, target_text_rva, top_k)?);
    }
    Ok(reports)
}

fn scan_one(
    donor: &DonorFunction,
    target_text: &[u8],
    target_text_rva: u64,
    top_k: usize,
) -> Result<ScanReport> {
    let donor_start = parse_hex_u64(&donor.rva_start)?;
    let donor_end = parse_hex_u64(&donor.rva_end)?;
    let donor_len = donor_end.saturating_sub(donor_start);

    // For each anchor, find every occurrence in the target. A candidate
    // function start is then `hit_offset - anchor.offset_in_function`. We
    // tally these candidate starts across all anchors; whichever start gets
    // the most votes is the best match.
    let mut tally: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();

    for anchor in &donor.anchors {
        let bytes = decode_hex(&anchor.bytes_hex)?;
        if bytes.is_empty() {
            continue;
        }
        for hit in memchr::memmem::find_iter(target_text, &bytes) {
            // Candidate start is signed because hits early in target with a
            // large `offset_in_function` would underflow.
            let candidate_start = (hit as i64) - (anchor.offset_in_function as i64);
            *tally.entry(candidate_start).or_insert(0) += 1;
        }
    }

    let mut sorted: Vec<(i64, usize)> = tally.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    let total_anchors = donor.anchors.len();
    let top_candidates = sorted
        .into_iter()
        .filter(|(start, _)| *start >= 0 && (*start as u64) + donor_len <= target_text.len() as u64)
        .take(top_k)
        .map(|(start, votes)| {
            let start_u = start as u64;
            let target_rva_start = target_text_rva + start_u;
            let target_rva_end = target_rva_start + donor_len;
            Candidate {
                rva_start_hex: format!("0x{target_rva_start:x}"),
                rva_end_hex: format!("0x{target_rva_end:x}"),
                matched_anchors: votes,
                total_anchors,
                score: votes as f32 / total_anchors.max(1) as f32,
            }
        })
        .collect();

    Ok(ScanReport {
        donor_name: donor.name.clone(),
        donor_rva_start: donor.rva_start.clone(),
        donor_rva_end: donor.rva_end.clone(),
        anchor_count: donor.anchors.len(),
        top_candidates,
    })
}

/// Read the donor JSON file produced by `scripts/ghidra/export_decoders.py`.
pub fn load_donors(path: &Path) -> Result<DonorDump> {
    let bytes = std::fs::read(path)?;
    let dump: DonorDump = serde_json::from_slice(&bytes)?;
    Ok(dump)
}

fn parse_hex_u64(s: &str) -> Result<u64> {
    u64::from_str_radix(s.trim_start_matches("0x"), 16).map_err(|e| {
        RoflError::Io(std::io::Error::other(format!("invalid hex u64 {s:?}: {e}")))
    })
}

fn decode_hex(s: &str) -> Result<Vec<u8>> {
    // Strip optional separators so the Ghidra script can output either
    // "4889c3..." or "48 89 c3 ..." without the parser caring.
    let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if cleaned.len() % 2 != 0 {
        return Err(RoflError::Io(std::io::Error::other(format!(
            "hex string {s:?} has odd length"
        ))));
    }
    let mut out = Vec::with_capacity(cleaned.len() / 2);
    for i in (0..cleaned.len()).step_by(2) {
        let byte = u8::from_str_radix(&cleaned[i..i + 2], 16).map_err(|e| {
            RoflError::Io(std::io::Error::other(format!(
                "invalid hex byte at {i} in {s:?}: {e}"
            )))
        })?;
        out.push(byte);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_donor_in_synthetic_target() {
        // Synthetic donor: a 4-byte function starting at RVA 0x1000.
        // Anchors are the first byte and the third byte.
        let donor = DonorFunction {
            name: "fake_decoder".to_string(),
            rva_start: "0x1000".to_string(),
            rva_end: "0x1004".to_string(),
            anchors: vec![
                Anchor {
                    offset_in_function: 0,
                    bytes_hex: "de".to_string(),
                },
                Anchor {
                    offset_in_function: 2,
                    bytes_hex: "ad".to_string(),
                },
            ],
        };

        // Target: 0x40 bytes of filler, then the function pattern, then more
        // filler. text_rva = 0x2000, so the function should land at
        // RVA 0x2000 + 0x40 = 0x2040.
        let mut target = vec![0u8; 0x40];
        target.extend_from_slice(&[0xde, 0xff, 0xad, 0xff]);
        target.extend_from_slice(&[0u8; 0x40]);

        let dump = DonorDump {
            patch: "test".to_string(),
            text_rva: "0x0".to_string(),
            functions: vec![donor],
        };

        let reports = scan_all(&dump, &target, 0x2000, 5).unwrap();
        assert_eq!(reports.len(), 1);
        let top = &reports[0].top_candidates[0];
        assert_eq!(top.rva_start_hex, "0x2040");
        assert_eq!(top.matched_anchors, 2);
        assert_eq!(top.total_anchors, 2);
    }

    #[test]
    fn rejects_odd_hex() {
        let r = decode_hex("abc");
        assert!(r.is_err());
    }
}
