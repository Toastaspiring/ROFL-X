//! Per-patch emulator configuration.
//!
//! Ported from Mowokuma's `src/emulator/config.rs` (commit 7181c9a). The
//! zip archive layout (`result.json` + `text.bin` / `data.bin` /
//! `rdata.bin`), the set of RVAs and struct offsets required, and the
//! hardcoded image base (`0x7ff76afd0000`) are all her reverse-
//! engineering work. See CREDITS.md.

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use serde_json::Value;

use crate::error::{Result, RoflError};

/// Default image base used when mapping PE sections into the emulator VM.
/// Matches the value Mowokuma hardcoded in her port.
pub const DEFAULT_BASE_ADDR: u64 = 0x7ff76afd0000;

/// One mapped PE section (`.text`, `.data`, or `.rdata`).
#[derive(Clone)]
pub struct Section {
    pub name: &'static str,
    pub rva: u64,
    pub size: u64,
    pub raw: Vec<u8>,
}

/// Parameters for the ward-spawn decode function in the game binary.
#[derive(Clone)]
pub struct WardSpawnDecrypt {
    pub netid: u32,
    pub rva: u64,
    pub end_rva: u64,
    pub id_offset: u64,
    pub owner_id_offset: u64,
    pub name_offset: u64,
    pub name_len_offset: u64,
    pub x_offset: u64,
    pub x_write_count: u32,
    pub y_offset: u64,
    pub y_write_count: u32,
}

/// Parameters for the movement/position decode function.
#[derive(Clone)]
pub struct MovDecrypt {
    pub netid: u32,
    pub rva: u64,
    pub end_rva: u64,
    pub payload_offset: u64,
    pub payload_size_offset: u64,
}

/// Everything the emulator needs for one game patch.
#[derive(Clone)]
pub struct Config {
    pub alloc1: u64,
    pub alloc2: u64,
    pub skip: u64,
    pub base_addr: u64,
    pub player_id_start: u32,
    pub ward_spawn_decrypt: WardSpawnDecrypt,
    pub mov_decrypt: MovDecrypt,
    pub text: Arc<Section>,
    pub data: Arc<Section>,
    pub rdata: Arc<Section>,
}

impl Config {
    /// Load a `.patch` archive and parse its `result.json` + section blobs.
    pub fn parse(patch_file: &Path) -> Result<Self> {
        let zipfile = File::open(patch_file)?;
        let mut archive = zip::ZipArchive::new(zipfile).map_err(|e| {
            RoflError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("patch archive open: {e}"),
            ))
        })?;

        let mut result_bytes = Vec::new();
        archive
            .by_name("result.json")
            .map_err(|e| {
                RoflError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("patch archive missing result.json: {e}"),
                ))
            })?
            .read_to_end(&mut result_bytes)?;
        let json: Value = serde_json::from_slice(&result_bytes)?;

        let raw_text = read_zip_entry(&mut archive, "text.bin")?;
        let raw_data = read_zip_entry(&mut archive, "data.bin")?;
        let raw_rdata = read_zip_entry(&mut archive, "rdata.bin")?;

        let text = Section {
            name: "text",
            rva: str_hex_to_u64(get_str(&json, "text", "rva")?)?,
            size: get_u64(&json, "text", "size")?,
            raw: raw_text,
        };
        let data = Section {
            name: "data",
            rva: str_hex_to_u64(get_str(&json, "data", "rva")?)?,
            size: get_u64(&json, "data", "size")?,
            raw: raw_data,
        };
        let rdata = Section {
            name: "rdata",
            rva: str_hex_to_u64(get_str(&json, "rdata", "rva")?)?,
            size: get_u64(&json, "rdata", "size")?,
            raw: raw_rdata,
        };

        let ws = json
            .get("ward_spawn_decrypt")
            .ok_or(RoflError::MetadataMissingKey("ward_spawn_decrypt"))?;
        let ward_spawn_decrypt = WardSpawnDecrypt {
            netid: ws
                .get("netid")
                .and_then(Value::as_u64)
                .ok_or(RoflError::MetadataMissingKey("ward_spawn_decrypt.netid"))?
                as u32,
            rva: str_hex_to_u64(as_str_at(ws, "rva_start")?)?,
            end_rva: str_hex_to_u64(as_str_at(ws, "rva_end")?)?,
            id_offset: str_hex_to_u64(as_str_at(ws, "id_offset")?)?,
            owner_id_offset: str_hex_to_u64(as_str_at(ws, "owner_id_offset")?)?,
            name_offset: str_hex_to_u64(as_str_at(ws, "name_offset")?)?,
            name_len_offset: str_hex_to_u64(as_str_at(ws, "name_len_offset")?)?,
            x_offset: str_hex_to_u64(as_str_at(ws, "x_offset")?)?,
            x_write_count: str_hex_to_u32(as_str_at(ws, "x_write_count")?)?,
            y_offset: str_hex_to_u64(as_str_at(ws, "y_offset")?)?,
            y_write_count: str_hex_to_u32(as_str_at(ws, "y_write_count")?)?,
        };

        let mv = json
            .get("mov_decrypt")
            .ok_or(RoflError::MetadataMissingKey("mov_decrypt"))?;
        let mov_decrypt = MovDecrypt {
            netid: mv
                .get("netid")
                .and_then(Value::as_u64)
                .ok_or(RoflError::MetadataMissingKey("mov_decrypt.netid"))?
                as u32,
            rva: str_hex_to_u64(as_str_at(mv, "rva_start")?)?,
            end_rva: str_hex_to_u64(as_str_at(mv, "rva_end")?)?,
            payload_offset: str_hex_to_u64(as_str_at(mv, "payload_offset")?)?,
            payload_size_offset: str_hex_to_u64(as_str_at(mv, "payload_size_offset")?)?,
        };

        Ok(Config {
            alloc1: str_hex_to_u64(get_top_str(&json, "alloc1_rva")?)?,
            alloc2: str_hex_to_u64(get_top_str(&json, "alloc2_rva")?)?,
            skip: str_hex_to_u64(get_top_str(&json, "skip_rva")?)?,
            base_addr: DEFAULT_BASE_ADDR,
            player_id_start: str_hex_to_u32(get_top_str(&json, "player_id_start")?)?,
            ward_spawn_decrypt,
            mov_decrypt,
            text: Arc::new(text),
            data: Arc::new(data),
            rdata: Arc::new(rdata),
        })
    }

    /// Pick the `.patch` file under `patch_dir` that matches the replay's
    /// short patch tag. Mowokuma's file-naming convention: she strips the
    /// leading "1" (her version-reading bug), replaces "." with "-", and
    /// drops the trailing "." or "-".
    ///
    /// We accept three candidate names per patch, in order:
    ///   "16-8.patch", "6-8.patch", "<patch>.patch"
    /// so we can read either Mowokuma's upstream archives or our own.
    pub fn resolve_patch_file(patch_dir: &Path, patch_tag: &str) -> Option<std::path::PathBuf> {
        let standard = format!("{}.patch", patch_tag.replace('.', "-"));
        let mowokuma = patch_tag
            .strip_prefix('1')
            .map(|rest| format!("{}.patch", rest.replace('.', "-")));

        for name in std::iter::once(standard.as_str())
            .chain(mowokuma.as_deref())
            .chain(std::iter::once(patch_tag))
        {
            let candidate = patch_dir.join(name);
            if candidate.exists() {
                return Some(candidate);
            }
        }
        None
    }
}

fn read_zip_entry(
    archive: &mut zip::ZipArchive<File>,
    name: &str,
) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    archive
        .by_name(name)
        .map_err(|e| {
            RoflError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("patch archive missing {name}: {e}"),
            ))
        })?
        .read_to_end(&mut buf)?;
    Ok(buf)
}

fn get_str<'a>(v: &'a Value, k1: &str, k2: &'static str) -> Result<&'a str> {
    v.get(k1)
        .and_then(|x| x.get(k2))
        .and_then(Value::as_str)
        .ok_or_else(|| RoflError::MetadataMissingKey(Box::leak(format!("{k1}.{k2}").into_boxed_str())))
}

fn get_u64(v: &Value, k1: &str, k2: &'static str) -> Result<u64> {
    v.get(k1)
        .and_then(|x| x.get(k2))
        .and_then(Value::as_u64)
        .ok_or_else(|| RoflError::MetadataMissingKey(Box::leak(format!("{k1}.{k2}").into_boxed_str())))
}

fn get_top_str<'a>(v: &'a Value, k: &'static str) -> Result<&'a str> {
    v.get(k)
        .and_then(Value::as_str)
        .ok_or(RoflError::MetadataMissingKey(k))
}

fn as_str_at<'a>(v: &'a Value, k: &'static str) -> Result<&'a str> {
    v.get(k)
        .and_then(Value::as_str)
        .ok_or(RoflError::MetadataMissingKey(k))
}

fn str_hex_to_u64(s: &str) -> Result<u64> {
    u64::from_str_radix(s.trim_start_matches("0x"), 16).map_err(|e| {
        RoflError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid hex u64 {s:?}: {e}"),
        ))
    })
}

fn str_hex_to_u32(s: &str) -> Result<u32> {
    u32::from_str_radix(s.trim_start_matches("0x"), 16).map_err(|e| {
        RoflError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid hex u32 {s:?}: {e}"),
        ))
    })
}
