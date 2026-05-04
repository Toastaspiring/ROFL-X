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

/// How the movement/position decoder writes its result, so the post-decode
/// step knows where to read the decoded data from.
///
/// On 5-5 (and earlier) the decoder allocates a buffer (via `alloc1` /
/// `alloc2`) and writes a `(pointer, size)` pair into the output struct.
/// The buffer contains a varint-encoded waypoint stream, parsed by
/// `PathPacket::parse_buffer_stream`.
///
/// On 16.9+ Riot moved to inline writes — the final position lands as
/// two `f32`s directly in the output struct at known offsets, with no
/// separate buffer. The waypoint list lives in a callee-allocated vector
/// at separate offsets that we don't yet decode.
#[derive(Clone, Debug, Default)]
pub enum MovOutputFormat {
    /// Mowokuma's 5-5-era format: `(payload_offset, payload_size_offset)`
    /// point to a `(buf_ptr, size)` pair; the buffer holds varint-encoded
    /// waypoints. This is the default for backward compatibility with
    /// existing `.patch` archives.
    #[default]
    BufferStream,
    /// 16.9+ inline-floats format: the decoder writes the final position
    /// as two `f32`s at `inline_x_offset` and `inline_y_offset`. No
    /// waypoint parsing — the packet carries one position per call.
    InlineFloats {
        inline_x_offset: u64,
        inline_y_offset: u64,
    },
}

/// Parameters for the movement/position decode function.
#[derive(Clone, Default)]
pub struct MovDecrypt {
    pub netid: u32,
    pub rva: u64,
    pub end_rva: u64,
    /// `BufferStream`: file offset where decoded buf-ptr lands.
    /// `InlineFloats`: ignored.
    pub payload_offset: u64,
    /// `BufferStream`: file offset where decoded buf-size lands.
    /// `InlineFloats`: ignored.
    pub payload_size_offset: u64,
    pub format: MovOutputFormat,
}

/// One generic extra-decoder entry. Used to map any netid to a decoder
/// function and dump its output struct writes. Supports the long tail of
/// unknown packet classes — for each one, we don't yet have a typed
/// struct or field semantics, but we can still capture every write the
/// decoder makes and surface it in the JSON output for downstream
/// inspection.
///
/// This is the extension point for "every packet decoded": as new
/// classes get RE'd, they migrate from `extra_decoders` (raw dump) to
/// dedicated typed structs (full semantics).
#[derive(Clone, Debug)]
pub struct ExtraDecoder {
    /// Symbolic name shown in the output JSON, e.g. `"replication"`.
    pub name: String,
    /// Packet id (netid) on this patch.
    pub netid: u32,
    /// Function start RVA in the patched binary.
    pub rva_start: u64,
    /// Function end RVA (exclusive).
    pub rva_end: u64,
    /// How much of the output struct to capture as raw bytes (default 0x90).
    pub struct_size: u64,
    /// Optional hint for downstream consumers about what semantic class
    /// this maps to (e.g. `"Replication"`, `"UnitApplyDamage"`). Free-form.
    pub semantic_hint: Option<String>,
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
    /// Generic decoders for packet classes whose struct layout isn't yet
    /// reverse-engineered. Each one runs the decoder via the emulator and
    /// dumps the resulting struct writes as raw hex into the output JSON.
    pub extra_decoders: Vec<ExtraDecoder>,
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
        // Output format is optional. Without it (or with the explicit value
        // "buffer-stream") we use the 5-5 layout. With "inline-floats" we
        // read two f32s directly out of the struct at the given offsets.
        let format_str = mv
            .get("output_format")
            .and_then(Value::as_str)
            .unwrap_or("buffer-stream");
        let format = match format_str {
            "buffer-stream" => MovOutputFormat::BufferStream,
            "inline-floats" => MovOutputFormat::InlineFloats {
                inline_x_offset: str_hex_to_u64(as_str_at(mv, "inline_x_offset")?)?,
                inline_y_offset: str_hex_to_u64(as_str_at(mv, "inline_y_offset")?)?,
            },
            other => {
                return Err(RoflError::Io(std::io::Error::other(format!(
                    "unknown mov_decrypt.output_format {other:?} \
                     (expected \"buffer-stream\" or \"inline-floats\")"
                ))));
            }
        };
        let (payload_offset, payload_size_offset) = match &format {
            MovOutputFormat::BufferStream => (
                str_hex_to_u64(as_str_at(mv, "payload_offset")?)?,
                str_hex_to_u64(as_str_at(mv, "payload_size_offset")?)?,
            ),
            MovOutputFormat::InlineFloats { .. } => (0, 0),
        };
        let mov_decrypt = MovDecrypt {
            netid: mv
                .get("netid")
                .and_then(Value::as_u64)
                .ok_or(RoflError::MetadataMissingKey("mov_decrypt.netid"))?
                as u32,
            rva: str_hex_to_u64(as_str_at(mv, "rva_start")?)?,
            end_rva: str_hex_to_u64(as_str_at(mv, "rva_end")?)?,
            payload_offset,
            payload_size_offset,
            format,
        };

        // Optional extra_decoders array for the long-tail packet classes.
        // Each entry: { name, netid, rva_start, rva_end, struct_size?, semantic_hint? }
        let extra_decoders = match json.get("extra_decoders") {
            Some(Value::Array(arr)) => arr
                .iter()
                .map(|e| {
                    Ok(ExtraDecoder {
                        name: e
                            .get("name")
                            .and_then(Value::as_str)
                            .ok_or(RoflError::MetadataMissingKey("extra_decoders[].name"))?
                            .to_string(),
                        netid: e
                            .get("netid")
                            .and_then(Value::as_u64)
                            .ok_or(RoflError::MetadataMissingKey("extra_decoders[].netid"))?
                            as u32,
                        rva_start: str_hex_to_u64(as_str_at(e, "rva_start")?)?,
                        rva_end: str_hex_to_u64(as_str_at(e, "rva_end")?)?,
                        struct_size: e
                            .get("struct_size")
                            .and_then(Value::as_str)
                            .map(str_hex_to_u64)
                            .transpose()?
                            .unwrap_or(0x90),
                        semantic_hint: e
                            .get("semantic_hint")
                            .and_then(Value::as_str)
                            .map(String::from),
                    })
                })
                .collect::<Result<Vec<_>>>()?,
            _ => Vec::new(),
        };

        Ok(Config {
            alloc1: str_hex_to_u64(get_top_str(&json, "alloc1_rva")?)?,
            alloc2: str_hex_to_u64(get_top_str(&json, "alloc2_rva")?)?,
            skip: str_hex_to_u64(get_top_str(&json, "skip_rva")?)?,
            base_addr: DEFAULT_BASE_ADDR,
            player_id_start: str_hex_to_u32(get_top_str(&json, "player_id_start")?)?,
            ward_spawn_decrypt,
            mov_decrypt,
            extra_decoders,
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
