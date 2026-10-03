//! Format-2 `.patch` archives, written by `scripts/build_patch.py`.
//!
//! A format-1 archive (Mowokuma's, see `config.rs`) names two decoder
//! functions and the struct offsets to read back from them. A format-2
//! archive instead names the handful of game functions needed to run
//! *every* packet class through the game's own code: the netid factory,
//! the allocator, the base-class deserializer, plus the environment the
//! emulator has to fake (TLS template, CFG dispatch thunk, hidden import
//! slots, the team registry). Layout and rationale: `docs/DECODING.md`.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use serde_json::Value;

use crate::error::{Result, RoflError};

/// One PE section, mapped at `image_base + rva`.
#[derive(Clone)]
pub struct ImageSection {
    pub name: String,
    pub rva: u64,
    pub size: u64,
    pub raw: Vec<u8>,
}

/// Riot's name for one netid, from its std::function handler registration.
#[derive(Clone, Debug)]
pub struct PacketName {
    /// e.g. `PKT_NPC_Hero_Die_s`.
    pub class: String,
    /// The client class that registered the handler, e.g. `AIBaseClient`.
    pub client: String,
}

/// Global the enum readers range-check against; see `docs/DECODING.md`.
#[derive(Clone, Copy, Debug)]
pub struct TeamRegistry {
    pub global_rva: u64,
    pub count: u32,
}

/// Everything in a format-2 archive.
#[derive(Clone)]
pub struct PatchArchive {
    pub file_version: Option<String>,
    pub image_base: u64,
    pub image_size: u64,
    pub sections: Vec<ImageSection>,
    pub factory_rva: u64,
    pub max_netid: u32,
    pub base_deserialize_rva: u64,
    pub alloc_rva: u64,
    pub free_rvas: Vec<u64>,
    pub cfg_dispatch_rva: Option<u64>,
    pub tls_template_rva: u64,
    pub tls_size: usize,
    pub import_slots: Vec<u64>,
    pub team_registry: Option<TeamRegistry>,
    pub packets: BTreeMap<u16, Vec<PacketName>>,
}

impl PatchArchive {
    /// True when `result.json` inside the archive declares `"format": 2`.
    pub fn is_format2(path: &Path) -> Result<bool> {
        let mut archive = open_zip(path)?;
        let json = read_json(&mut archive)?;
        Ok(json.get("format").and_then(Value::as_u64) == Some(2))
    }

    pub fn load(path: &Path) -> Result<Self> {
        let mut archive = open_zip(path)?;
        let json = read_json(&mut archive)?;
        if json.get("format").and_then(Value::as_u64) != Some(2) {
            return Err(archive_err("not a format-2 archive"));
        }

        let mut sections = Vec::new();
        for s in array(&json, "sections")? {
            let file = str_at(s, "file")?;
            let mut raw = Vec::new();
            archive
                .by_name(file)
                .map_err(|e| archive_err(&format!("missing {file}: {e}")))?
                .read_to_end(&mut raw)?;
            sections.push(ImageSection {
                name: str_at(s, "name")?.to_string(),
                rva: hex_at(s, "rva")?,
                size: hex_at(s, "size")?,
                raw,
            });
        }

        let factory = json.get("factory").ok_or(RoflError::MetadataMissingKey("factory"))?;
        let tls = json.get("tls").ok_or(RoflError::MetadataMissingKey("tls"))?;
        let team_registry = match json.get("team_registry") {
            Some(Value::Object(_)) => {
                let t = &json["team_registry"];
                Some(TeamRegistry {
                    global_rva: hex_at(t, "global")?,
                    count: t.get("count").and_then(Value::as_u64).unwrap_or(4) as u32,
                })
            }
            _ => None,
        };

        let mut packets = BTreeMap::new();
        if let Some(Value::Object(map)) = json.get("packets") {
            for (netid, names) in map {
                let netid: u16 = netid
                    .parse()
                    .map_err(|_| archive_err(&format!("bad netid key {netid:?}")))?;
                let names = names
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|n| {
                                Some(PacketName {
                                    class: n.get("class")?.as_str()?.to_string(),
                                    client: n.get("client")?.as_str()?.to_string(),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                packets.insert(netid, names);
            }
        }

        Ok(PatchArchive {
            file_version: json
                .get("binary")
                .and_then(|b| b.get("file_version"))
                .and_then(Value::as_str)
                .map(str::to_string),
            image_base: hex_at(&json, "image_base")?,
            image_size: hex_at(&json, "image_size")?,
            sections,
            factory_rva: hex_at(factory, "rva")?,
            max_netid: factory
                .get("max_netid")
                .and_then(Value::as_u64)
                .ok_or(RoflError::MetadataMissingKey("factory.max_netid"))? as u32,
            base_deserialize_rva: hex_at(&json, "base_deserialize")?,
            alloc_rva: hex_at(&json, "alloc")?,
            free_rvas: array(&json, "free")?
                .iter()
                .map(|v| parse_hex(v.as_str().unwrap_or("")))
                .collect::<Result<_>>()?,
            cfg_dispatch_rva: match json.get("cfg_dispatch").and_then(Value::as_str) {
                Some(s) => Some(parse_hex(s)?),
                None => None,
            },
            tls_template_rva: hex_at(tls, "template")?,
            tls_size: tls
                .get("size")
                .and_then(Value::as_u64)
                .ok_or(RoflError::MetadataMissingKey("tls.size"))? as usize,
            import_slots: array(&json, "import_slots")?
                .iter()
                .map(|v| parse_hex(v.as_str().unwrap_or("")))
                .collect::<Result<_>>()?,
            team_registry,
            packets,
        })
    }

    /// Riot class name for `netid`, if one handler registration names it.
    /// A netid registered by several client classes keeps one class name
    /// (the classes always agree in format-2 archives built so far).
    pub fn class_of(&self, netid: u16) -> Option<&str> {
        self.packets
            .get(&netid)
            .and_then(|names| names.first())
            .map(|n| n.class.as_str())
    }

    /// Bytes of the image at `rva` (zero-filled past a section's raw data).
    pub fn read(&self, rva: u64, len: usize) -> Option<Vec<u8>> {
        let s = self
            .sections
            .iter()
            .find(|s| rva >= s.rva && rva < s.rva + s.size)?;
        let start = (rva - s.rva) as usize;
        let mut out: Vec<u8> = s.raw.iter().skip(start).take(len).copied().collect();
        out.resize(len, 0);
        Some(out)
    }
}

fn open_zip(path: &Path) -> Result<zip::ZipArchive<File>> {
    zip::ZipArchive::new(File::open(path)?)
        .map_err(|e| archive_err(&format!("open {}: {e}", path.display())))
}

fn read_json(archive: &mut zip::ZipArchive<File>) -> Result<Value> {
    let mut bytes = Vec::new();
    archive
        .by_name("result.json")
        .map_err(|e| archive_err(&format!("missing result.json: {e}")))?
        .read_to_end(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn array<'a>(v: &'a Value, k: &'static str) -> Result<&'a Vec<Value>> {
    v.get(k)
        .and_then(Value::as_array)
        .ok_or(RoflError::MetadataMissingKey(k))
}

fn str_at<'a>(v: &'a Value, k: &'static str) -> Result<&'a str> {
    v.get(k)
        .and_then(Value::as_str)
        .ok_or(RoflError::MetadataMissingKey(k))
}

fn hex_at(v: &Value, k: &'static str) -> Result<u64> {
    parse_hex(str_at(v, k)?)
}

fn parse_hex(s: &str) -> Result<u64> {
    u64::from_str_radix(s.trim_start_matches("0x"), 16)
        .map_err(|e| archive_err(&format!("invalid hex {s:?}: {e}")))
}

fn archive_err(msg: &str) -> RoflError {
    RoflError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("patch archive: {msg}"),
    ))
}
