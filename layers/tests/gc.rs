//! Tests for `gc_iteration` under the new GC logic:
//!
//! Two entry types, one rule per type:
//!
//! - `Session` is removed iff:
//!     1. `get_attached == false` (no GET stream is attached), AND
//!     2. `created.elapsed() - last_used > timeout`
//!
//! - `PairEntry` (stream-up pairing) is removed iff:
//!     1. `done == true` (GET/POST were paired and delivered), OR
//!     2. `done == false` but `created.elapsed() > timeout` (a zombie:
//!        parked half that was never paired).
//!
//! `last_used` is a millisecond offset since `created`, i.e. the age of the
//! last activity. `created.elapsed() - last_used.ms()` is the time since the
//! last activity. Both entries are aged in tests by backdating `created`.
//! `PairEntry` cannot be constructed manually (its `state` is private), so a
//! fresh one comes from `PairEntry::new()`.

#[cfg(test)]
mod tests {
    use cray_layers::transport::xhttp2::server::{PairEntry, Session, gc_iteration};
    use dashmap::DashMap;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use tokio::time::{Duration, Instant};

    fn session_created_ago(secs: u64) -> Arc<Session> {
        let mut s = Session::new();
        s.created = Instant::now() - Duration::from_secs(secs);
        Arc::new(s)
    }

    fn pair_entry_ago(secs: u64) -> Arc<PairEntry> {
        let mut p = PairEntry::new();
        p.created = Instant::now() - Duration::from_secs(secs);
        Arc::new(p)
    }

    fn pair_entry_done_now() -> Arc<PairEntry> {
        let p = PairEntry::new();
        p.done.store(true, Ordering::Relaxed);
        Arc::new(p)
    }

    #[tokio::test]
    async fn test_gc_removes_expired_sessions() {
        let cache = DashMap::new();
        let mut vec = Vec::new();

        // Created 40s ago, never used (last_used == 0), no GET attached:
        // idle for the full 40s -> expired at 30s timeout.
        let session_expired = session_created_ago(40);

        // Created just now: idle for ~0s -> kept.
        let session_fresh = Arc::new(Session::new());

        cache.insert("expired_session_id".to_string(), session_expired);
        cache.insert("fresh_session_id".to_string(), session_fresh);

        assert_eq!(cache.len(), 2);

        let timeout = Duration::from_secs(30);
        gc_iteration(&cache, timeout, &mut vec).await;

        assert_eq!(cache.len(), 1);
        assert!(!cache.contains_key("expired_session_id"));
        assert!(cache.contains_key("fresh_session_id"));
    }

    #[tokio::test]
    async fn test_gc_keeps_recently_used_sessions() {
        let cache = DashMap::new();
        let mut vec = Vec::new();

        // Created 60s ago, last used 20s ago (last_used == 40_000ms):
        // 60s - 40s = 20s of idleness, below the 30s timeout -> kept.
        let session = session_created_ago(60);
        session.last_used.store(40_000, Ordering::Relaxed);
        cache.insert("active_id".to_string(), session);

        let timeout = Duration::from_secs(30);
        gc_iteration(&cache, timeout, &mut vec).await;

        assert_eq!(cache.len(), 1);
        assert!(cache.contains_key("active_id"));
    }

    #[tokio::test]
    async fn test_gc_keeps_get_attached_sessions() {
        let cache = DashMap::new();
        let mut vec = Vec::new();

        // Created 60s ago and fully idle, but a GET stream is attached to it:
        // the get_attached gate outranks the timeout -> kept.
        let session = session_created_ago(60);
        session.get_attached.store(true, Ordering::Relaxed);
        cache.insert("attached_id".to_string(), session);

        let timeout = Duration::from_secs(30);
        gc_iteration(&cache, timeout, &mut vec).await;

        assert_eq!(cache.len(), 1);
        assert!(cache.contains_key("attached_id"));
    }

    #[tokio::test]
    async fn test_gc_reuses_vec_and_clears_it() {
        let cache = DashMap::new();
        let mut vec = vec!["stale_entry".to_string()];

        let session = session_created_ago(40);
        cache.insert("expired_id".to_string(), session);

        let timeout = Duration::from_secs(30);
        gc_iteration(&cache, timeout, &mut vec).await;

        // vec was reused for this round and no longer holds the stale entry.
        assert_eq!(vec.len(), 1);
        assert_eq!(vec[0], "expired_id");
        assert!(cache.is_empty());
    }

    #[tokio::test]
    async fn test_gc_pair_dropped_even_when_fresh() {
        let cache = DashMap::new();
        let mut vec = Vec::new();

        // Pair was delivered (done == true) just now: drop regardless of age.
        cache.insert("delivered_id".to_string(), pair_entry_done_now());

        let timeout = Duration::from_secs(30);
        gc_iteration(&cache, timeout, &mut vec).await;

        assert!(cache.is_empty());
        assert_eq!(vec[0], "delivered_id");
    }

    #[tokio::test]
    async fn test_gc_pair_zombie_dropped_after_timeout() {
        let cache = DashMap::new();
        let mut vec = Vec::new();

        // Parked but never delivered (done == false), created 40s ago:
        // zombie -> dropped at the 30s timeout.
        cache.insert("zombie_id".to_string(), pair_entry_ago(40));

        let timeout = Duration::from_secs(30);
        gc_iteration(&cache, timeout, &mut vec).await;

        assert!(cache.is_empty());
        assert_eq!(vec[0], "zombie_id");
    }

    #[tokio::test]
    async fn test_gc_pair_keeps_active_pairing_window() {
        let cache = DashMap::new();
        let mut vec = Vec::new();

        // Fresh parked pair (done == false, created just now): GET and POST
        // are still within the pairing window -> kept.
        cache.insert("pairing_id".to_string(), Arc::new(PairEntry::new()));

        let timeout = Duration::from_secs(30);
        gc_iteration(&cache, timeout, &mut vec).await;

        assert_eq!(cache.len(), 1);
        assert!(cache.contains_key("pairing_id"));
        assert!(vec.is_empty());
    }
}
