use crate::error::{Result, RoflError};

/// ASCII "RIOT" at the start of every `.rofl` file.
pub const MAGIC: [u8; 4] = *b"RIOT";

/// Byte offset of the u8 that gives the version-string length.
pub const VERSION_LEN_OFFSET: usize = 0x0E;

/// Byte offset at which the version string starts.
pub const VERSION_OFFSET: usize = VERSION_LEN_OFFSET + 1;

/// File header: fixed prefix plus a length-prefixed ASCII version string.
///
/// See `docs/ROFL_FORMAT.md` § Section A for the byte-level layout.
#[derive(Debug, Clone)]
pub struct FileHeader {
    /// `u16` LE at offset 0x04. Observed value 2 in sample_a; likely a
    /// format-version field, but not branched on until we see a second value.
    pub format_version: u16,

    /// `u16` LE at offset 0x06. Value 217 in sample_a. Meaning unknown;
    /// preserved for round-trip fidelity.
    pub field_u16_0x06: u16,

    /// Six bytes at offset 0x08. High entropy; possibly game id, session
    /// tag, or per-file nonce. Preserved for round-trip fidelity.
    pub field_bytes_0x08: [u8; 6],

    /// Full version string as stored in the header, e.g. `"16.8.766.8562"`.
    pub version: String,

    /// Total header size in bytes (`VERSION_OFFSET + version.len()`).
    pub size: usize,
}

impl FileHeader {
    /// Parse the header from the start of a whole-file buffer.
    pub fn parse(buffer: &[u8]) -> Result<Self> {
        if buffer.len() < VERSION_OFFSET {
            return Err(RoflError::FileTooShort {
                got: buffer.len(),
                need: VERSION_OFFSET,
            });
        }

        let magic: [u8; 4] = buffer[0..4].try_into().unwrap();
        if magic != MAGIC {
            return Err(RoflError::InvalidMagic { got: magic });
        }

        let format_version = u16::from_le_bytes([buffer[4], buffer[5]]);
        let field_u16_0x06 = u16::from_le_bytes([buffer[6], buffer[7]]);
        let field_bytes_0x08: [u8; 6] = buffer[8..14].try_into().unwrap();

        let version_len = buffer[VERSION_LEN_OFFSET] as usize;
        let version_end = VERSION_OFFSET + version_len;
        if buffer.len() < version_end {
            return Err(RoflError::FileTooShort {
                got: buffer.len(),
                need: version_end,
            });
        }

        let version_bytes = &buffer[VERSION_OFFSET..version_end];
        let version = std::str::from_utf8(version_bytes)
            .map_err(|_| RoflError::VersionNotUtf8)?
            .to_string();

        Ok(FileHeader {
            format_version,
            field_u16_0x06,
            field_bytes_0x08,
            version,
            size: version_end,
        })
    }

    /// Short patch tag derived from the version string.
    ///
    /// For `"16.8.766.8562"` returns `"16.8"`. If the version does not have
    /// at least two dot-separated parts, returns the whole version string.
    pub fn patch(&self) -> &str {
        let mut dots = 0;
        for (i, c) in self.version.char_indices() {
            if c == '.' {
                dots += 1;
                if dots == 2 {
                    return &self.version[..i];
                }
            }
        }
        &self.version
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// First 32 bytes of sample_a (sha256 prefix 0224a2d7f1ff0cc3).
    /// Reproducible via `scripts/sample_walkthrough.py`.
    const SAMPLE_A_HEADER: [u8; 32] = [
        0x52, 0x49, 0x4F, 0x54, 0x02, 0x00, 0xD9, 0x00, 0x7E, 0xE2, 0x68, 0x69, 0x86, 0xA9, 0x0D,
        0x31, 0x36, 0x2E, 0x38, 0x2E, 0x37, 0x36, 0x36, 0x2E, 0x38, 0x35, 0x36, 0x32, 0x01, 0x00,
        0x00, 0x00,
    ];

    #[test]
    fn parses_sample_a_header() {
        let h = FileHeader::parse(&SAMPLE_A_HEADER).unwrap();
        assert_eq!(h.format_version, 2);
        assert_eq!(h.field_u16_0x06, 217);
        assert_eq!(h.field_bytes_0x08, [0x7E, 0xE2, 0x68, 0x69, 0x86, 0xA9]);
        assert_eq!(h.version, "16.8.766.8562");
        assert_eq!(h.size, 0x1C);
        assert_eq!(h.patch(), "16.8");
    }

    #[test]
    fn rejects_wrong_magic() {
        let mut bytes = SAMPLE_A_HEADER;
        bytes[0] = b'X';
        let err = FileHeader::parse(&bytes).unwrap_err();
        matches!(err, RoflError::InvalidMagic { .. });
    }

    #[test]
    fn rejects_short_buffer() {
        let err = FileHeader::parse(&[0u8; 4]).unwrap_err();
        matches!(err, RoflError::FileTooShort { .. });
    }

    #[test]
    fn rejects_truncated_version() {
        // length prefix says 13 but buffer ends at 0x14.
        let bytes = &SAMPLE_A_HEADER[..0x14];
        let err = FileHeader::parse(bytes).unwrap_err();
        matches!(err, RoflError::FileTooShort { .. });
    }

    #[test]
    fn patch_handles_minimal_versions() {
        let h = FileHeader {
            format_version: 0,
            field_u16_0x06: 0,
            field_bytes_0x08: [0; 6],
            version: "15".to_string(),
            size: 0,
        };
        assert_eq!(h.patch(), "15");

        let h = FileHeader {
            version: "15.4".to_string(),
            ..h
        };
        assert_eq!(h.patch(), "15.4");

        let h = FileHeader {
            version: "16.8.766.8562".to_string(),
            ..h
        };
        assert_eq!(h.patch(), "16.8");
    }
}
