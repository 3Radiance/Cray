use tracing_subscriber::{EnvFilter, fmt};

#[macro_export]
macro_rules! log_tag {
    ($level:ident, $tag:expr, $($arg:tt)*) => {
        tracing::$level!(target: $tag, "[{}] {}", $tag, format_args!($($arg)*))
    };
}

pub fn default_log_filter() -> EnvFilter {
    EnvFilter::try_new("debug").expect("valid tracing env filter")
}

pub fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| default_log_filter());

    let _ = fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_level(true)
        .with_timer(tracing_subscriber::fmt::time::ChronoLocal::rfc_3339())
        .event_format(tracing_subscriber::fmt::format().pretty())
        .try_init();
}

#[cfg(test)]
mod tests {
    #[test]
    fn default_log_filter_is_debug() {
        let filter = super::default_log_filter();

        assert_eq!(filter.to_string(), "debug");
    }

    #[test]
    fn logging_smoke_test() {
        super::init_logging();
        crate::log_tag!(info, "TCP", "smoke test message");
    }
}
