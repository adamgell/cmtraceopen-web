//! Deterministic-rotation device picker. Given a list of device_ids and
//! a fire's start-minute salt, compute sha256(device_id || salt) and pick
//! the K devices with the smallest hash. Stable within a fire (re-runs
//! pick the same K), uniform-ish across fires.

use sha2::{Digest, Sha256};

/// Pick `k` devices from `device_ids` using the deterministic-rotation
/// algorithm. Returns the picked `device_id`s in ascending-hash order.
pub fn pick(device_ids: &[String], k: usize, salt: i64) -> Vec<String> {
    if k == 0 || device_ids.is_empty() {
        return vec![];
    }
    let mut scored: Vec<(u64, &String)> = device_ids
        .iter()
        .map(|id| (hash(id, salt), id))
        .collect();
    scored.sort_by_key(|(h, id)| (*h, (*id).clone()));
    scored.iter().take(k).map(|(_, id)| (*id).clone()).collect()
}

fn hash(device_id: &str, salt: i64) -> u64 {
    let mut h = Sha256::new();
    h.update(device_id.as_bytes());
    h.update(salt.to_le_bytes());
    let digest = h.finalize();
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    u64::from_le_bytes(bytes)
}

/// Salt for a fire = the start-minute of `last_fired_at` (or `created_at`
/// if never fired). Sub-daily schedules rotate between fires; daily
/// schedules rotate between days.
pub fn salt(last_fired_at: chrono::DateTime<chrono::Utc>) -> i64 {
    last_fired_at.timestamp() / 60
}

/// K = ceil(total * rate_pct / 100). Always at least 1 if total > 0 and
/// rate_pct > 0.
pub fn target_count(total: usize, rate_pct: u32) -> usize {
    if total == 0 || rate_pct == 0 {
        return 0;
    }
    ((total as u64 * rate_pct as u64).div_ceil(100)) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ids(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("dev-{i:04}")).collect()
    }

    #[test]
    fn deterministic_within_a_fire() {
        let pool = ids(1000);
        let s = salt(chrono::Utc.with_ymd_and_hms(2026, 4, 30, 12, 0, 0).unwrap());
        let p1 = pick(&pool, 100, s);
        let p2 = pick(&pool, 100, s);
        assert_eq!(p1, p2, "same salt picks the same set");
    }

    #[test]
    fn rotates_across_minutes() {
        let pool = ids(1000);
        let s1 = salt(chrono::Utc.with_ymd_and_hms(2026, 4, 30, 12, 0, 0).unwrap());
        let s2 = salt(chrono::Utc.with_ymd_and_hms(2026, 4, 30, 13, 0, 0).unwrap());
        let p1 = pick(&pool, 100, s1);
        let p2 = pick(&pool, 100, s2);
        let overlap = p1.iter().filter(|id| p2.contains(id)).count();
        assert!(overlap < 30, "expected near-uniform rotation, overlap was {overlap}");
    }

    #[test]
    fn target_count_is_ceil() {
        assert_eq!(target_count(50, 10), 5);
        assert_eq!(target_count(51, 10), 6);
        assert_eq!(target_count(0, 10), 0);
        assert_eq!(target_count(10, 0), 0);
        assert_eq!(target_count(3, 10), 1);
    }

    #[test]
    fn k_zero_returns_empty() {
        let pool = ids(10);
        assert!(pick(&pool, 0, 1).is_empty());
    }

    #[test]
    fn empty_pool_returns_empty() {
        assert!(pick(&[], 5, 1).is_empty());
    }

    #[test]
    fn fairness_chi_square() {
        let pool = ids(1000);
        let mut counts: std::collections::HashMap<String, u32> = Default::default();
        for minute in 0..1000 {
            let picks = pick(&pool, 100, minute);
            for id in picks { *counts.entry(id).or_insert(0) += 1; }
        }
        let too_few = counts.values().filter(|&&c| c < 50).count();
        let too_many = counts.values().filter(|&&c| c > 150).count();
        assert!(too_few + too_many < pool.len() / 5,
            "too many outliers: too_few={too_few} too_many={too_many}");
    }
}
