#[cfg(test)]
mod tests {
    use cray_layers::transport::xhttp2::server::{Session, gc_iteration};
    use dashmap::DashMap;
    use std::sync::Arc;
    use tokio::time::{Duration, Instant};

    #[tokio::test]
    async fn test_gc_removes_expired_sessions() {
        let cache = DashMap::new();
        let mut vec = Vec::new();

        let session_expired = Arc::new(Session::new());
        {
            let mut last_used = session_expired.last_used.lock().await;
            *last_used = Instant::now() - Duration::from_secs(40);
        }

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
    async fn test_gc_keeps_active_sessions() {
        let cache = DashMap::new();
        let mut vec = Vec::new();

        let session = Arc::new(Session::new());
        cache.insert("active_id".to_string(), session);

        let timeout = Duration::from_secs(30);
        gc_iteration(&cache, timeout, &mut vec).await;

        assert_eq!(cache.len(), 1);
        assert!(cache.contains_key("active_id"));
    }
}
