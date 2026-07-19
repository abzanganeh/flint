//! Deterministic question-order shuffle (session-seeded, no `rand` dependency).

use uuid::Uuid;

/// Seed for Fisher–Yates shuffle derived from a session id.
pub fn session_shuffle_seed(id: Uuid) -> u64 {
    let b = id.as_bytes();
    let lo = u64::from_le_bytes(b[0..8].try_into().expect("uuid lo"));
    let hi = u64::from_le_bytes(b[8..16].try_into().expect("uuid hi"));
    lo ^ hi.rotate_left(17)
}

/// Deterministic per-item sort key: same (item, seed) always produces the
/// same key, regardless of what other items are present or absent. This is
/// what makes shuffle order *stable* under insertion/removal — removing one
/// question never reorders the rest.
pub fn stable_shuffle_key(item: &str, seed: u64) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325 ^ seed;
    for byte in item.trim().to_lowercase().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Fisher–Yates shuffle of `items` using `seed` (stable for the same seed).
///
/// Retained for `mock::conductor` (base-question order for mock interviews,
/// not the pending-bank list). Not used for the question bank — see
/// `stable_shuffle_key` for that.
pub fn shuffle_strings(items: &mut [String], seed: u64) {
    if items.len() < 2 {
        return;
    }
    let mut state = seed ^ 0x9E37_79B9_7F4A_7C15;
    for i in (1..items.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let j = (state as usize) % (i + 1);
        items.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shuffle_is_deterministic_per_seed() {
        let mut a = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        let mut b = a.clone();
        shuffle_strings(&mut a, 42);
        shuffle_strings(&mut b, 42);
        assert_eq!(a, b);
        assert_ne!(a, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn stable_shuffle_key_is_unaffected_by_other_items() {
        let seed = 42u64;
        let questions = vec!["a", "b", "c", "d", "e"];
        let keys_before: Vec<u64> = questions
            .iter()
            .map(|q| stable_shuffle_key(q, seed))
            .collect();
        // Remove "b" — every other item's key must be identical, proving
        // order among survivors cannot change when one item leaves the set.
        let remaining = ["a", "c", "d", "e"];
        for q in remaining {
            let key = stable_shuffle_key(q, seed);
            let original_index = questions.iter().position(|x| *x == q).unwrap();
            assert_eq!(key, keys_before[original_index]);
        }
    }

    #[test]
    fn shuffle_preserves_multiset() {
        let original = vec!["x".into(), "y".into(), "z".into()];
        let mut shuffled = original.clone();
        shuffle_strings(&mut shuffled, 99);
        let mut a = shuffled.clone();
        let mut b = original;
        a.sort();
        b.sort();
        assert_eq!(a, b);
    }
}
