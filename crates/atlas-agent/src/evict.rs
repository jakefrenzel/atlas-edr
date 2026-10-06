//! Bounded maps evict in batches: when a map passes its cap, the oldest eighth
//! goes in one pass. Evicting one entry per insert would scan the whole map on
//! every insert once it is full (O(n) each, at thousands of events a second).

/// The keys of the `len / 8` (at least one) entries with the smallest `age`.
pub fn oldest<K: Copy, A: Ord + Copy>(entries: impl Iterator<Item = (K, A)>, len: usize) -> Vec<K> {
    let mut all: Vec<(A, K)> = entries.map(|(k, a)| (a, k)).collect();
    let n = (len / 8).max(1).min(all.len());
    if n == 0 {
        return Vec::new();
    }
    if n < all.len() {
        all.select_nth_unstable_by_key(n - 1, |(a, _)| *a);
    }
    all.truncate(n);
    all.into_iter().map(|(_, k)| k).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_oldest_eighth() {
        let entries = (0..16u32).map(|k| (k, 100 - k)); // key 15 is the oldest
        let mut got = oldest(entries, 16);
        got.sort_unstable();
        assert_eq!(got, [14, 15]);
        assert_eq!(oldest((0..3u32).map(|k| (k, k)), 3), [0]);
        assert!(oldest(std::iter::empty::<(u32, u32)>(), 0).is_empty());
    }
}
