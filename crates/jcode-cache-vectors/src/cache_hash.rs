//! Hash computation for individual cache vectors.
//!
//! Uses djb2 -- fast, deterministic, dependency-free.

use serde::{Deserialize, Serialize};

/// A 64-bit hash identifying a single cache vector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VectorHash(pub u64);

impl VectorHash {
    pub const ZERO: Self = Self(0);
}

/// djb2 string hash returning a raw `u64`.
pub fn djb2_hash(s: &str) -> u64 {
    let mut h: u64 = 5381;
    for &b in s.as_bytes() {
        h = h.wrapping_mul(33).wrapping_add(b as u64);
    }
    h
}

/// Hash arbitrary serializable data via deterministic JSON + djb2.
///
/// Serialization of these types in practice is infallible (plain data
/// structures such as prompt bodies, tool schemas, and config records), so
/// a JSON serialization failure is treated as an empty-document fallback
/// (`djb2("") == 5381`) rather than a panic. The fallback keeps cache
/// lookups progressing instead of crashing the call site.
pub fn compute_hash(data: &impl Serialize) -> u64 {
    // Matched explicitly rather than via unwrap_or_default so the fallback is
    // visible at the call site. This crate has no logging dependency, and a
    // failed serialization is a degenerate-input case rather than an
    // operational fault, so the empty-document hash is the whole behaviour.
    let json = match serde_json::to_string(data) {
        Ok(json) => json,
        Err(_) => String::new(),
    };
    djb2_hash(&json)
}

/// Hash a byte slice, returning a [`VectorHash`].
pub fn hash_bytes(data: &[u8]) -> VectorHash {
    let mut h: u64 = 5381;
    for &b in data {
        h = h.wrapping_mul(33).wrapping_add(b as u64);
    }
    VectorHash(h)
}

/// Hash a string (UTF-8 bytes), returning a [`VectorHash`].
pub fn hash_str(s: &str) -> VectorHash {
    hash_bytes(s.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn djb2_known_values() {
        // djb2("") = seed
        assert_eq!(djb2_hash(""), 5381);
        // djb2("hello") verified against runtime output
        assert_eq!(djb2_hash("hello"), 210_714_636_441);
    }

    #[test]
    fn djb2_deterministic_and_unique() {
        assert_eq!(djb2_hash("abc"), djb2_hash("abc"));
        assert_ne!(djb2_hash("abc"), djb2_hash("abd"));
        assert_ne!(djb2_hash("ab"), djb2_hash("ba"));
    }

    #[test]
    fn compute_hash_consistency() {
        let v = vec![1u32, 2, 3];
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(compute_hash(&v), djb2_hash(&json));
    }

    #[test]
    fn compute_hash_different_data() {
        #[derive(serde::Serialize)]
        struct S {
            x: i32,
        }
        assert_ne!(compute_hash(&S { x: 1 }), compute_hash(&S { x: 2 }));
        assert_eq!(compute_hash(&S { x: 1 }), compute_hash(&S { x: 1 }));
    }

    #[test]
    fn vector_hash_wrapper_matches() {
        assert_eq!(hash_str("hello"), VectorHash(djb2_hash("hello")));
        assert_eq!(VectorHash::ZERO.0, 0);
    }
}
