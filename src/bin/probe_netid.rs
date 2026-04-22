//! One-shot exploration tool: for each candidate class descriptor entry
//! in Mowokuma's 5-5 `.rdata`, try calling each of its u64 fields as a
//! "get-netid" thunk and see if any returns a plausible u16 netid.
//!
//! If we find a consistent field slot across our two known entries
//! (ward@0xe3d7b0=571, mov@0xe45710=980), we have a full netid-to-
//! decoder mapping for every packet class in the game.
//!
//! Ran once, findings documented in docs/RE_PATCH.md. Not part of the
//! normal rofl-x CLI because it's a one-off reverse-engineering probe.

use std::path::PathBuf;

use rofl_x::emulator::config::Config;
use rofl_x::emulator::StubEmulator;

const WARD_ENTRY_OFF: u64 = 0xe1ab8;
const MOV_ENTRY_OFF: u64 = 0xe1060;
const EXPECTED_WARD_NETID: u64 = 571;
const EXPECTED_MOV_NETID: u64 = 980;

fn read_entry_fields(rdata: &[u8], offset: u64) -> [u64; 5] {
    let mut fields = [0u64; 5];
    for i in 0..5 {
        let start = offset as usize + i * 8;
        let bytes: [u8; 8] = rdata[start..start + 8].try_into().unwrap();
        fields[i] = u64::from_le_bytes(bytes);
    }
    fields
}

fn in_text(rva: u64) -> bool {
    (0x1000..(0x1000 + 23_699_456)).contains(&rva)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: probe-netid <patch-zip> [--enumerate]");
        std::process::exit(1);
    }
    let patch = PathBuf::from(&args[1]);
    let enumerate = args.iter().any(|a| a == "--enumerate");
    let config = Config::parse(&patch).expect("load patch archive");
    let rdata_bytes = &config.rdata.raw;

    if !enumerate {
        // Probe just ward + mov entries, show each slot's return value.
        let ward_fields = read_entry_fields(rdata_bytes, WARD_ENTRY_OFF);
        let mov_fields = read_entry_fields(rdata_bytes, MOV_ENTRY_OFF);
        println!(
            "ward entry fields: {:?}",
            ward_fields.map(|f| format!("0x{f:x}"))
        );
        println!(
            "mov  entry fields: {:?}",
            mov_fields.map(|f| format!("0x{f:x}"))
        );

        let ward_entry_va = config.rdata.rva + WARD_ENTRY_OFF + config.base_addr;
        let mov_entry_va = config.rdata.rva + MOV_ENTRY_OFF + config.base_addr;

        for slot in 0..5 {
            let ward_candidate = ward_fields[slot];
            let mov_candidate = mov_fields[slot];
            if !in_text(ward_candidate) || !in_text(mov_candidate) {
                println!("slot {slot}: not .text in both, skipping");
                continue;
            }
            println!(
                "\nslot {slot}: ward_fn=0x{ward_candidate:x} mov_fn=0x{mov_candidate:x}"
            );
            for (label, fn_rva, this_ptr, expected) in [
                ("ward", ward_candidate, ward_entry_va, EXPECTED_WARD_NETID),
                ("mov", mov_candidate, mov_entry_va, EXPECTED_MOV_NETID),
            ] {
                let mut emu = StubEmulator::new(config.clone());
                if emu.setup().is_err() {
                    continue;
                }
                let result = emu.call_thiscall_returning_u64(fn_rva, fn_rva + 0x100, this_ptr);
                match result {
                    Some(rax) => {
                        let low16 = rax & 0xffff;
                        let note = if low16 == expected { " MATCH" } else { "" };
                        println!(
                            "  {label}: rax=0x{rax:x} low16={low16}{note} (expected {expected})"
                        );
                    }
                    None => println!("  {label}: call failed"),
                }
            }
        }
        return;
    }

    // Enumerate mode: scan rdata for every 48-byte entry ending in the
    // sentinel, call slot 1 and slot 4 on each, record the answers.
    let sentinel: [u8; 8] = [0xff, 0xff, 0xb9, 0x85, 0x08, 0x80, 0xff, 0xff];
    let mut entries: Vec<u64> = Vec::new();
    let mut i = 0;
    while i + 48 <= rdata_bytes.len() {
        if &rdata_bytes[i + 40..i + 48] == sentinel {
            entries.push(i as u64);
        }
        i += 1;
    }
    println!("{} entries with sentinel-at-40", entries.len());

    // Filter to entries whose slot 0 is a plausible .text RVA.
    entries.retain(|o| {
        let f = read_entry_fields(rdata_bytes, *o);
        in_text(f[0])
    });
    println!("{} entries whose field[0] is in .text range", entries.len());

    // For speed: sample a spread across the list, plus always the two known ones.
    let sample_stride = (entries.len() / 80).max(1);
    let sampled: Vec<u64> = entries.iter().step_by(sample_stride).copied().collect();
    println!("sampling {} entries", sampled.len());

    println!();
    println!(
        "{:>10} {:>12} {:>10} {:>10} {:>10}",
        "rdata_off", "decoder_rva", "slot1", "slot4", "note"
    );
    for off in &sampled {
        let f = read_entry_fields(rdata_bytes, *off);
        let this_va = config.rdata.rva + off + config.base_addr;
        let mut slot1 = String::from("?");
        let mut slot4 = String::from("?");
        for (slot, out) in [(1usize, &mut slot1), (4usize, &mut slot4)] {
            if in_text(f[slot]) {
                let mut emu = StubEmulator::new(config.clone());
                if emu.setup().is_ok() {
                    if let Some(v) =
                        emu.call_thiscall_returning_u64(f[slot], f[slot] + 0x100, this_va)
                    {
                        *out = format!("{}", v & 0xffff);
                    } else {
                        *out = String::from("trap");
                    }
                } else {
                    *out = String::from("setup-fail");
                }
            } else {
                *out = String::from("not-text");
            }
        }
        let note = if *off == WARD_ENTRY_OFF {
            "WARD (nid=571)"
        } else if *off == MOV_ENTRY_OFF {
            "MOV  (nid=980)"
        } else {
            ""
        };
        println!(
            "0x{:08x} 0x{:010x} {:>10} {:>10}  {}",
            off, f[0], slot1, slot4, note
        );
    }
}
