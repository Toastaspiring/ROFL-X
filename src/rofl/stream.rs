/// Logical stream that a chunk belongs to.
///
/// Encoded in the high byte of the chunk's `chunk_id_2` field:
/// `(chunk_id_2 >> 24) & 0xFF`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StreamTag {
    /// Game-chunk stream: periodic delta packets, driving gameplay state.
    GameChunk,
    /// Keyframe stream: periodic full-state snapshots.
    Keyframe,
    /// Singleton initial-keyframe marker (first chunk in the file).
    StartKeyframe,
    /// Singleton transition marker that sits between the initial keyframe
    /// and the first game chunk. Has `compressed_len == 0`; its
    /// `uncompressed_len` bytes are an opaque 17-byte body.
    StartSentinel,
    /// Any other high-byte value we haven't catalogued. Passed through so
    /// unknown streams don't crash the walker.
    Unknown(u8),
}

impl StreamTag {
    /// Decode from a `chunk_id_2` u32 value by inspecting the high byte.
    pub fn from_chunk_id_2(id2: u32) -> Self {
        match ((id2 >> 24) & 0xFF) as u8 {
            0x01 => Self::GameChunk,
            0x02 => Self::Keyframe,
            0x03 => Self::StartKeyframe,
            0x04 => Self::StartSentinel,
            other => Self::Unknown(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_known_tags() {
        assert_eq!(StreamTag::from_chunk_id_2(0x01000000), StreamTag::GameChunk);
        assert_eq!(StreamTag::from_chunk_id_2(0x02000000), StreamTag::Keyframe);
        assert_eq!(
            StreamTag::from_chunk_id_2(0x03000000),
            StreamTag::StartKeyframe
        );
        assert_eq!(
            StreamTag::from_chunk_id_2(0x04000000),
            StreamTag::StartSentinel
        );
    }

    #[test]
    fn passes_through_unknown_tag() {
        assert_eq!(
            StreamTag::from_chunk_id_2(0xFF000000),
            StreamTag::Unknown(0xFF)
        );
    }

    #[test]
    fn ignores_low_bytes() {
        // Low bytes of chunk_id_2 are always zero in sample_a; if they ever
        // aren't, the high byte still controls the stream tag.
        assert_eq!(StreamTag::from_chunk_id_2(0x0100ABCD), StreamTag::GameChunk);
    }
}
