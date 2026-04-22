use thiserror::Error;

/// Every failure mode the parser can emit.
///
/// We fold per-layer errors into one enum so the CLI and test harness both
/// have a single type to pattern-match on. Each variant carries enough
/// context to be reproducible from the message alone.
#[derive(Error, Debug)]
pub enum RoflError {
    #[error("file too short: have {got} bytes, need at least {need}")]
    FileTooShort { got: usize, need: usize },

    #[error("invalid RIOT magic: got {got:02x?}, expected [0x52, 0x49, 0x4F, 0x54]")]
    InvalidMagic { got: [u8; 4] },

    #[error("invalid UTF-8 in version string")]
    VersionNotUtf8,

    #[error("invalid UTF-8 in metadata JSON")]
    MetadataNotUtf8,

    #[error("invalid metadata JSON: {0}")]
    MetadataJson(#[from] serde_json::Error),

    #[error("missing expected metadata key: {0}")]
    MetadataMissingKey(&'static str),

    #[error("zstd decompression failed in chunk {chunk_id}: {message}")]
    Decompression { chunk_id: u32, message: String },

    #[error("chunk {chunk_id} body extends past the chunks region boundary")]
    ChunkBoundsExceeded { chunk_id: u32 },

    #[error(
        "decompressed chunk {chunk_id}: expected {expected} bytes, got {actual}"
    )]
    DecompressedLenMismatch {
        chunk_id: u32,
        expected: u32,
        actual: usize,
    },

    #[error("block framing error at offset {offset}: {message}")]
    BlockFraming { offset: usize, message: String },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, RoflError>;
