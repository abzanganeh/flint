//! Deterministic question-order shuffle (session-seeded, no `rand` dependency).

use uuid::Uuid;

/// Seed for Fisher–Yates shuffle derived from a session id.
pub fn session_shuffle_seed(id: Uuid) -> u64 {
    let b = id.as_bytes();
    let lo = u64::from_le_bytes(b[0..8].try_into().expect("uuid lo"));
    let hi = u64::from_le_bytes(b[8..16].try_into().expect("uuid hi"));
    lo ^ hi.rotate_left(17)
}

/// Fisher–Yates shuffle of `items` using `seed` (stable for the same seed).
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
