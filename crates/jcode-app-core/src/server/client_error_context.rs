use std::fmt;

#[derive(Debug)]
pub(super) struct DirectEventWrite {
    kind: std::io::ErrorKind,
    raw_os_error: Option<i32>,
}

impl DirectEventWrite {
    pub(super) fn new(error: &std::io::Error) -> Self {
        Self {
            kind: error.kind(),
            raw_os_error: error.raw_os_error(),
        }
    }
}

impl fmt::Display for DirectEventWrite {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "operation=write_direct_event io_error_kind={:?} raw_os_error={:?}",
            self.kind, self.raw_os_error,
        )
    }
}

pub(super) fn initial_lightweight_error_context(
    error: &anyhow::Error,
    request_kind: &str,
    request_id: u64,
) -> String {
    let operation = if error.downcast_ref::<DirectEventWrite>().is_some() {
        "write_direct_event"
    } else {
        "handle_lightweight_control"
    };
    let io_error = error.downcast_ref::<std::io::Error>();
    format!(
        "phase=initial_lightweight_control request_kind={request_kind} request_id={request_id} operation={operation} io_error_kind={:?} raw_os_error={:?}",
        io_error.map(std::io::Error::kind),
        io_error.and_then(std::io::Error::raw_os_error),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_control_context_omits_sensitive_error_messages() {
        let native = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "private-io-sentinel");
        let operation = DirectEventWrite::new(&native);
        let error = anyhow::Error::new(native)
            .context("private-context-sentinel")
            .context(operation);
        assert!(error.to_string().contains("io_error_kind=BrokenPipe"));
        assert!(!error.to_string().contains("private-"));
        let context = initial_lightweight_error_context(&error, "ping", 42);
        let error = error.context(context);
        let logged = error.to_string();
        assert!(logged.contains("request_kind=ping request_id=42"));
        assert!(logged.contains("operation=write_direct_event"));
        assert!(logged.contains("io_error_kind=Some(BrokenPipe)"));
        assert!(!logged.contains("private-"));
        assert!(error.downcast_ref::<std::io::Error>().is_some());

        let error = anyhow::anyhow!("private-handler-sentinel");
        let context = initial_lightweight_error_context(&error, "comm_list", 43);
        assert!(context.contains("operation=handle_lightweight_control"));
        assert!(context.contains("io_error_kind=None raw_os_error=None"));
        assert!(!context.contains("private-"));
    }

    #[test]
    fn unwrapped_write_context_preserves_safe_os_metadata() {
        let native = std::io::Error::from_raw_os_error(232);
        let expected = format!(
            "operation=write_direct_event io_error_kind={:?} raw_os_error=Some(232)",
            native.kind(),
        );
        let operation = DirectEventWrite::new(&native);
        let error = anyhow::Error::new(native).context(operation);
        assert_eq!(error.to_string(), expected);
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .raw_os_error(),
            Some(232)
        );
    }
}
