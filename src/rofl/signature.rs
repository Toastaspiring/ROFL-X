/// Length of the signature block, in bytes.
pub const SIGNATURE_SIZE: usize = 0x100; // 256

/// The 256-byte opaque block that sits between the last chunk and the
/// metadata JSON.
///
/// Contents are almost certainly an RSA signature over the preceding file
/// bytes. We do not verify and we do not modify; the sole contract is
/// round-trip preservation.
#[derive(Debug, Clone)]
pub struct Signature {
    pub bytes: [u8; SIGNATURE_SIZE],
}

impl Signature {
    /// Copy 256 bytes from `slice` into a new `Signature`. Returns `None`
    /// when the input is the wrong length.
    pub fn from_slice(slice: &[u8]) -> Option<Self> {
        if slice.len() != SIGNATURE_SIZE {
            return None;
        }
        let mut bytes = [0u8; SIGNATURE_SIZE];
        bytes.copy_from_slice(slice);
        Some(Self { bytes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_wrong_length() {
        assert!(Signature::from_slice(&[0u8; 255]).is_none());
        assert!(Signature::from_slice(&[0u8; 257]).is_none());
    }

    #[test]
    fn roundtrips_exact_length() {
        let input: Vec<u8> = (0..=255u8).collect();
        let sig = Signature::from_slice(&input).unwrap();
        assert_eq!(&sig.bytes[..], &input[..]);
    }
}
