//! Modified XXTEA used for NMS metadata (`mf_*.hg`) files.
//!
//! The variant differs from textbook XXTEA in two ways:
//!
//! * the round count is fixed (8 for the 104-byte vanilla metadata layout,
//!   6 for every later layout) instead of `6 + 52 / n`, and
//! * the key's first word is derived from the storage slot while words 1..3
//!   are fixed.
//!
//! Everything is wrapping 32-bit arithmetic over little-endian words. The
//! cipher is deterministic: encrypting an unchanged word array reproduces the
//! original file bytes exactly, which is what makes "patch the fields you
//! understand, keep the rest" a safe strategy.

use crate::metadata::Slot;

/// Delta constant (golden ratio, `2^32 / phi`).
const DELTA: u32 = 0x9E37_79B9;
/// Wrapping complement of [`DELTA`], used to step backwards.
const REVERSE_DELTA: u32 = 0x61C8_8647;

/// Fixed key words 1..3: the bytes `"EVAD"`, `"NAYR"`, `"TNRG"` little-endian.
const KEY_REST: [u32; 3] = [0x4441_5645, 0x5259_414E, 0x4752_4E54];

/// Key derivation: XOR constant applied to the slot index.
const KEY_XOR: u32 = 0x1422_CB8C;
/// Key derivation: rotate amount.
const KEY_ROTATE: u32 = 13;
/// Key derivation: multiplier.
const KEY_MULTIPLY: u32 = 5;
/// Key derivation: addend.
const KEY_ADD: u32 = 0xE654_6B64;

/// Derive the four-word key for a storage slot.
///
/// `key[0] = rotate_left(slot ^ 0x1422CB8C, 13) * 5 + 0xE6546B64` (all
/// wrapping); words 1..3 are fixed. A slot is a storage-file index, not one of
/// the 15 user-visible paired save slots.
pub fn derive_key(slot: Slot) -> [u32; 4] {
    let k0 = (u32::from(slot.index()) ^ KEY_XOR)
        .rotate_left(KEY_ROTATE)
        .wrapping_mul(KEY_MULTIPLY)
        .wrapping_add(KEY_ADD);
    [k0, KEY_REST[0], KEY_REST[1], KEY_REST[2]]
}

/// Decrypt `words` in place with `rounds` rounds.
pub fn decrypt(words: &mut [u32], key: &[u32; 4], rounds: usize) {
    let last = words.len() - 1;
    let mut hash = DELTA.wrapping_mul(rounds as u32);

    for _ in 0..rounds {
        let key_index = ((hash >> 2) & 3) as usize;
        let mut current = words[0];

        for j in (1..=last).rev() {
            let prev = words[j - 1];
            let t1 = (current >> 3) ^ (prev << 4);
            let t2 = current.wrapping_mul(4) ^ (prev >> 5);
            let t3 = prev ^ key[(j & 3) ^ key_index];
            let t4 = current ^ hash;
            words[j] = words[j].wrapping_sub(t1.wrapping_add(t2) ^ t3.wrapping_add(t4));
            current = words[j];
        }

        let prev = words[last];
        let t1 = (current >> 3) ^ (prev << 4);
        let t2 = current.wrapping_mul(4) ^ (prev >> 5);
        let t3 = prev ^ key[key_index];
        let t4 = current ^ hash;
        words[0] = words[0].wrapping_sub(t1.wrapping_add(t2) ^ t3.wrapping_add(t4));

        hash = hash.wrapping_add(REVERSE_DELTA);
    }
}

/// Encrypt `words` in place with `rounds` rounds; inverse of [`decrypt`].
pub fn encrypt(words: &mut [u32], key: &[u32; 4], rounds: usize) {
    let last = words.len() - 1;
    let mut hash: u32 = 0;

    for _ in 0..rounds {
        hash = hash.wrapping_add(DELTA);
        let key_index = ((hash >> 2) & 3) as usize;

        let next = words[1];
        let prev = words[last];
        let t1 = (next >> 3) ^ (prev << 4);
        let t2 = next.wrapping_mul(4) ^ (prev >> 5);
        let t3 = prev ^ key[key_index];
        let t4 = next ^ hash;
        words[0] = words[0].wrapping_add(t1.wrapping_add(t2) ^ t3.wrapping_add(t4));

        for j in 1..=last {
            let next = if j == last { words[0] } else { words[j + 1] };
            let prev = words[j - 1];
            let t1 = (next >> 3) ^ (prev << 4);
            let t2 = next.wrapping_mul(4) ^ (prev >> 5);
            let t3 = prev ^ key[(j & 3) ^ key_index];
            let t4 = next ^ hash;
            words[j] = words[j].wrapping_add(t1.wrapping_add(t2) ^ t3.wrapping_add(t4));
        }
    }
}

/// Convert bytes to little-endian words. Trailing bytes that do not fill a word
/// are ignored; every known metadata length is a multiple of four.
pub fn words_from_bytes(bytes: &[u8]) -> Vec<u32> {
    bytes.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect()
}

/// Convert words back to little-endian bytes.
pub fn words_to_bytes(words: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(words.len() * 4);
    for w in words {
        out.extend_from_slice(&w.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_words(n: usize) -> Vec<u32> {
        (0..n as u32).map(|i| i.wrapping_mul(0x9E37_79B9).rotate_left(i % 31)).collect()
    }

    #[test]
    fn round_trips_for_every_slot_and_layout() {
        for index in 0u8..=31 {
            let slot = Slot::from_index(index).unwrap();
            let key = derive_key(slot);
            for (words_len, rounds) in [(26usize, 8usize), (90, 6), (96, 6), (108, 6)] {
                let original = sample_words(words_len);
                let mut words = original.clone();
                encrypt(&mut words, &key, rounds);
                assert_ne!(words, original, "ciphertext must differ (slot {index})");
                decrypt(&mut words, &key, rounds);
                assert_eq!(words, original, "decrypt must invert encrypt (slot {index}, {words_len} words)");
            }
        }
    }

    #[test]
    fn key_derivation_matches_known_value() {
        // PlayerState1 is storage index 2; the derived word is a published
        // constant from the community format documentation.
        assert_eq!(derive_key(Slot::player_state(1))[0], 0xA58D_37F8);
        let expected = (2u32 ^ KEY_XOR).rotate_left(KEY_ROTATE).wrapping_mul(KEY_MULTIPLY).wrapping_add(KEY_ADD);
        assert_eq!(derive_key(Slot::player_state(1))[0], expected);
    }

    #[test]
    fn words_bytes_round_trip() {
        let words = sample_words(37);
        assert_eq!(words_from_bytes(&words_to_bytes(&words)), words);
    }
}
