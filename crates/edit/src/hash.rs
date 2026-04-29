// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Provides fast, non-cryptographic hash functions.

use std::hash::Hasher;

/// A [`Hasher`] implementation for the wyhash algorithm.
///
/// NOTE that you DO NOT want to use this for hashing mere strings/slices.
/// The stdlib [`Hash`] implementation for them calls [`Hasher::write`] twice,
/// once for the contents and once for a length prefix / `0xff` suffix.
#[derive(Default, Clone, Copy)]
pub struct WyHash(u64);

impl Hasher for WyHash {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        self.0 = hash(self.0, bytes);
    }
}

/// The venerable wyhash hash function.
///
/// It's fast, has good statistical properties, and is in the public domain.
/// See: <https://github.com/wangyi-fudan/wyhash>
/// If you visit the link, you'll find that it was superseded by "rapidhash",
/// but that's not particularly interesting for this project. rapidhash results
/// in way larger assembly and isn't faster when hashing small amounts of data.
pub fn hash(mut seed: u64, data: &[u8]) -> u64 {
    unsafe {
        const S0: u64 = 0xa0761d6478bd642f;
        const S1: u64 = 0xe7037ed1a0b428db;
        const S2: u64 = 0x8ebc6af09c88c6e3;
        const S3: u64 = 0x589965cc75374cc3;

        let len = data.len();
        let mut p = data.as_ptr();
        let a;
        let b;

        seed ^= S0;

        if len <= 16 {
            if len >= 4 {
                a = (wyr4(p) << 32) | wyr4(p.add((len >> 3) << 2));
                b = (wyr4(p.add(len - 4)) << 32) | wyr4(p.add(len - 4 - ((len >> 3) << 2)));
            } else if len > 0 {
                a = wyr3(p, len);
                b = 0;
            } else {
                a = 0;
                b = 0;
            }
        } else {
            let mut i = len;
            if i > 48 {
                let mut seed1 = seed;
                let mut seed2 = seed;
                while {
                    seed = wymix(wyr8(p) ^ S1, wyr8(p.add(8)) ^ seed);
                    seed1 = wymix(wyr8(p.add(16)) ^ S2, wyr8(p.add(24)) ^ seed1);
                    seed2 = wymix(wyr8(p.add(32)) ^ S3, wyr8(p.add(40)) ^ seed2);
                    p = p.add(48);
                    i -= 48;
                    i > 48
                } {}
                seed ^= seed1 ^ seed2;
            }
            while i > 16 {
                seed = wymix(wyr8(p) ^ S1, wyr8(p.add(8)) ^ seed);
                i -= 16;
                p = p.add(16);
            }
            a = wyr8(p.offset(i as isize - 16));
            b = wyr8(p.offset(i as isize - 8));
        }

        wymix(S1 ^ (len as u64), wymix(a ^ S1, b ^ seed))
    }
}

unsafe fn wyr3(p: *const u8, k: usize) -> u64 {
    let p0 = unsafe { p.read() as u64 };
    let p1 = unsafe { p.add(k >> 1).read() as u64 };
    let p2 = unsafe { p.add(k - 1).read() as u64 };
    (p0 << 16) | (p1 << 8) | p2
}

unsafe fn wyr4(p: *const u8) -> u64 {
    unsafe { p.cast::<u32>().read_unaligned() as u64 }
}

unsafe fn wyr8(p: *const u8) -> u64 {
    unsafe { p.cast::<u64>().read_unaligned() }
}

// This is a weak mix function on its own. It may be worth considering
// replacing external uses of this function with a stronger one.
// On the other hand, it's very fast.
pub fn wymix(lhs: u64, rhs: u64) -> u64 {
    let lhs = lhs as u128;
    let rhs = rhs as u128;
    let r = lhs * rhs;
    (r >> 64) as u64 ^ (r as u64)
}

pub fn hash_str(seed: u64, s: &str) -> u64 {
    hash(seed, s.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_does_not_panic() {
        let _ = hash(0, b"");
        let _ = hash(0xdead_beef, b"");
    }

    #[test]
    fn deterministic() {
        for seed in [0u64, 1, 42, 0xdead_beef, u64::MAX] {
            for data in [b"".as_ref(), b"a", b"hello, world", &[0u8; 100][..]] {
                assert_eq!(hash(seed, data), hash(seed, data));
            }
        }
    }

    #[test]
    fn distinct_inputs_distinct_outputs() {
        let s = 0xa1b2_c3d4_e5f6_0718;
        let h1 = hash(s, b"abc");
        let h2 = hash(s, b"abd");
        assert_ne!(h1, h2, "near-identical inputs should hash differently");
    }

    #[test]
    fn distinct_seeds_distinct_outputs() {
        let data = b"the quick brown fox jumps over the lazy dog";
        assert_ne!(hash(0, data), hash(1, data));
        assert_ne!(hash(0xaa, data), hash(0x55, data));
    }

    #[test]
    fn hash_str_matches_hash_bytes() {
        for s in ["", "a", "abc", "héllo", "💀"] {
            assert_eq!(hash_str(0, s), hash(0, s.as_bytes()));
            assert_eq!(hash_str(7, s), hash(7, s.as_bytes()));
        }
    }

    #[test]
    fn variable_lengths_do_not_panic() {
        let buf: Vec<u8> = (0..256u32).map(|i| i as u8).collect();
        for n in 0..=buf.len() {
            let _ = hash(0, &buf[..n]);
        }
    }

    #[test]
    fn wyhash_write_finish_matches_hash() {
        let mut h = WyHash::default();
        Hasher::write(&mut h, b"hello world");
        assert_eq!(h.finish(), hash(0, b"hello world"));
    }

    #[test]
    fn wyhash_default_finish_is_zero() {
        let h = WyHash::default();
        assert_eq!(h.finish(), 0);
    }

    #[test]
    fn wymix_zero_zero_is_zero() {
        assert_eq!(wymix(0, 0), 0);
    }

    #[test]
    fn wymix_deterministic() {
        assert_eq!(wymix(123, 456), wymix(123, 456));
    }

    #[test]
    fn wymix_symmetric() {
        // wymix folds high/low halves of u128 product via xor — multiplication is
        // commutative, so the result should not depend on argument order.
        for (a, b) in [(1u64, 2), (0xdeadbeef, 0x12345678), (u64::MAX, 1), (u64::MAX, u64::MAX)] {
            assert_eq!(wymix(a, b), wymix(b, a));
        }
    }

    #[test]
    fn distinguishes_lengths() {
        // Hash should distinguish "ab" from "abc" etc. (avalanche on length).
        let mut seen = std::collections::HashSet::new();
        for n in 0..32 {
            let data = vec![b'x'; n];
            assert!(seen.insert(hash(0, &data)), "collision at len={n}");
        }
    }
}
