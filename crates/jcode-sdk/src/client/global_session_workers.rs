use super::*;

pub(super) fn discover_global_sessions(
    parent: JcodeClient,
    control: Arc<GlobalEventControl>,
    interval: Duration,
) {
    loop {
        if control.stopped.load(Ordering::Acquire) {
            return;
        }
        let sessions = match parent
            .request_ok(ApiRequest::ListSessions {
                include_archived: true,
                limit: None,
            })
            .and_then(|frame| match frame.event {
                ApiEvent::Sessions { sessions } => Ok(sessions),
                other => Err(unexpected("sessions", &other)),
            }) {
            Ok(sessions) => sessions,
            Err(error) => {
                stop_global_stream(&control, Some(error));
                return;
            }
        };

        for session in sessions {
            if control.stopped.load(Ordering::Acquire) {
                return;
            }
            start_global_child(&parent, &control, session.session_id);
        }

        if interval.is_zero() {
            return;
        }
        let Ok(guard) = control.wake_lock.lock() else {
            stop_global_stream(
                &control,
                Some(Error::new(
                    ErrorKind::Transport,
                    "global event lock poisoned",
                )),
            );
            return;
        };
        let _ = control.wake.wait_timeout(guard, interval);
    }
}

pub(super) fn start_global_child(
    parent: &JcodeClient,
    control: &Arc<GlobalEventControl>,
    session_id: String,
) {
    if control
        .children
        .lock()
        .map(|children| children.contains_key(&session_id))
        .unwrap_or(true)
    {
        return;
    }

    let connection = if let Some(options) = &parent.ssh_options {
        let mut options = options.clone();
        options.client_name = format!("{}/global-events", parent.inner.client_name);
        #[cfg(unix)]
        let shared = parent.shared_ssh_transport();
        #[cfg(unix)]
        if let Some(shared) = shared {
            shared.connect()
        } else if parent
            .inner
            .ssh_process
            .as_ref()
            .is_some_and(|process| process.was_shared)
        {
            Err(Error::new(
                ErrorKind::Disconnected,
                "shared SSH parent channel is closed",
            ))
        } else {
            JcodeClient::connect_ssh(options)
        }
        #[cfg(not(unix))]
        JcodeClient::connect_ssh(options)
    } else {
        JcodeClient::connect(ConnectOptions {
            supports_user_response: false,
            socket_path: Some(parent.inner.socket_path.clone()),
            client_name: format!("{}/global-events", parent.inner.client_name),
            request_timeout: parent.inner.request_timeout,
            ensure_runtime: false,
        })
    };
    let child = match connection {
        Ok(child) => child,
        Err(error) => {
            stop_global_stream(control, Some(error));
            return;
        }
    };
    let stream = child.events(Some(&session_id));
    if let Err(error) = child.attach_session(&session_id) {
        if !matches!(
            error.kind,
            ErrorKind::Harness(jcode_harness_api::ErrorCode::UnknownSession)
        ) {
            stop_global_stream(control, Some(error));
        }
        return;
    }
    if control.stopped.load(Ordering::Acquire) {
        return;
    }
    if let Ok(mut children) = control.children.lock() {
        children.insert(session_id.clone(), child);
    } else {
        stop_global_stream(
            control,
            Some(Error::new(
                ErrorKind::Transport,
                "global event lock poisoned",
            )),
        );
        return;
    }

    let pump_control = Arc::clone(control);
    std::thread::spawn(move || {
        while let Some(event) = stream.next() {
            if pump_control.stopped.load(Ordering::Acquire) {
                break;
            }
            match pump_control.tx.try_send(event) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    stop_global_stream(
                        &pump_control,
                        Some(Error::new(
                            ErrorKind::EventBufferOverflow,
                            format!(
                                "global_events consumer fell behind {} buffered events",
                                pump_control.max_buffered_events
                            ),
                        )),
                    );
                    break;
                }
                Err(TrySendError::Disconnected(_)) => {
                    stop_global_stream(&pump_control, None);
                    break;
                }
            }
        }
        if let Ok(mut children) = pump_control.children.lock() {
            children.remove(&session_id);
        }
    });
}
