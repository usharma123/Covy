//! Bounded, bincode-v1-compatible persistence encoding.
//!
//! Packet28's local persisted artifacts historically used bincode 1's
//! fixed-width, little-endian representation. This crate preserves those bytes
//! while replacing the unmaintained Serde decoder with schema-derived decoding
//! that rejects excessive collection preallocation and trailing bytes.

extern crate wincode_impl as wincode;

pub use wincode_impl::{
    config, error, io, tag_encoding, ReadError, ReadResult, SchemaRead, SchemaWrite, TypeMeta,
    WriteError, WriteResult,
};

use wincode_impl::config::Configuration;
use wincode_impl::io::Cursor;

/// Maximum allocation that one decoded collection may request.
///
/// Existing index/cache artifacts can legitimately be large, so this is a
/// compatibility ceiling rather than a recommended artifact size.
pub const PREALLOCATION_LIMIT_BYTES: usize = 512 * 1024 * 1024;

type WireConfig = Configuration<true, PREALLOCATION_LIMIT_BYTES>;

const WIRE_CONFIG: WireConfig =
    Configuration::default().with_preallocation_size_limit::<PREALLOCATION_LIMIT_BYTES>();

/// Serializes a schema value using the historical bincode-v1 wire profile.
///
/// # Errors
///
/// Returns an error when the encoded size overflows or the output cannot be
/// allocated.
pub fn serialize<T>(value: &T) -> Result<Vec<u8>, WriteError>
where
    T: SchemaWrite<WireConfig, Src = T> + ?Sized,
{
    <T as wincode_impl::config::Serialize<WireConfig>>::serialize(value, WIRE_CONFIG)
}

/// Deserializes one complete historical bincode-v1 value.
///
/// The decoder rejects trailing bytes and collection lengths whose requested
/// preallocation exceeds [`PREALLOCATION_LIMIT_BYTES`].
///
/// # Errors
///
/// Returns an error for truncated, malformed, oversized, or trailing input.
pub fn deserialize<T>(bytes: &[u8]) -> Result<T, ReadError>
where
    T: for<'de> SchemaRead<'de, WireConfig, Dst = T>,
{
    let mut cursor = Cursor::new(bytes);
    let value =
        <T as wincode_impl::config::DeserializeOwned<WireConfig>>::deserialize_from(&mut cursor)?;
    if cursor.position() != bytes.len() {
        return Err(ReadError::Custom("trailing bytes"));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    #[derive(Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
    enum Kind {
        Empty,
        Pair(u32, String),
        Named { enabled: bool, values: Vec<i64> },
    }

    #[derive(Debug, PartialEq, SchemaRead, SchemaWrite)]
    struct Representative {
        version: u16,
        pointer_sized: usize,
        signed_wide: i128,
        score: f64,
        maybe: Option<String>,
        kinds: Vec<Kind>,
        ordered: BTreeMap<String, BTreeSet<String>>,
        hashed: BTreeMap<String, u64>,
        nested: Vec<Vec<Vec<u32>>>,
    }

    #[derive(Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
    struct LengthProbe {
        prefix: u16,
        values: Vec<String>,
    }

    fn representative() -> Representative {
        Representative {
            version: 3,
            pointer_sized: 65_537,
            signed_wide: -123_456_789_012_345_678_901,
            score: 0.625,
            maybe: Some("packet28".to_owned()),
            kinds: vec![
                Kind::Empty,
                Kind::Pair(42, "answer".to_owned()),
                Kind::Named {
                    enabled: true,
                    values: vec![-7, 0, 99],
                },
            ],
            ordered: BTreeMap::from([
                (
                    "a".to_owned(),
                    BTreeSet::from(["x".to_owned(), "y".to_owned()]),
                ),
                ("b".to_owned(), BTreeSet::new()),
            ]),
            hashed: BTreeMap::from([("left".to_owned(), 11), ("right".to_owned(), 22)]),
            nested: vec![vec![vec![1, 2], Vec::new()], vec![vec![3]]],
        }
    }

    fn legacy_bytes() -> Vec<u8> {
        // Frozen output from bincode 1.3.3's `serialize` for `representative`.
        let encoded = concat!(
            "03000100010000000000cb93c9d07e60b14ef9ffffffffffffff0000000000",
            "00e43f0108000000000000007061636b657432380300000000000000000000",
            "00010000002a0000000600000000000000616e737765720200000001030000",
            "0000000000f9ffffffffffffff000000000000000063000000000000000200",
            "000000000000010000000000000061020000000000000001000000000000",
            "007801000000000000007901000000000000006200000000000000000200",
            "00000000000004000000000000006c6566740b0000000000000005000000",
            "000000007269676874160000000000000002000000000000000200000000",
            "000000020000000000000001000000020000000000000000000000010000",
            "0000000000010000000000000003000000"
        );
        encoded
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let digits = std::str::from_utf8(pair).expect("fixture uses ASCII");
                u8::from_str_radix(digits, 16).expect("fixture uses hexadecimal digits")
            })
            .collect()
    }

    #[test]
    fn bytes_match_the_frozen_bincode_v1_profile() {
        let value = representative();
        let expected = legacy_bytes();

        assert_eq!(serialize(&value).unwrap(), expected);
        assert_eq!(deserialize::<Representative>(&expected).unwrap(), value);
    }

    #[test]
    fn rejects_truncation_and_trailing_bytes() {
        let bytes = legacy_bytes();
        assert!(deserialize::<Representative>(&bytes[..bytes.len() - 1]).is_err());

        let mut trailing = bytes;
        trailing.push(0);
        assert!(deserialize::<Representative>(&trailing).is_err());
    }

    #[test]
    fn rejects_forged_collection_lengths_before_allocation() {
        let mut forged = 1u16.to_le_bytes().to_vec();
        forged.extend_from_slice(&u64::MAX.to_le_bytes());

        let error = deserialize::<LengthProbe>(&forged).unwrap_err();
        assert!(
            matches!(error, ReadError::PreallocationSizeLimit { .. }),
            "{error:?}"
        );
    }
}
