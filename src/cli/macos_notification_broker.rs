//! Native macOS turn-notification broker.
//!
//! This module is entered only by the generated `Jcode Notifications.app`
//! bundle. It owns Notification Center identity/authorization, consumes the
//! durable inbox written by local TUI clients, and activates the route embedded
//! in a notification when the user clicks it.

#[cfg(target_os = "macos")]
mod platform {
    use std::ptr::NonNull;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};

    use anyhow::{Context, Result};
    use objc2::rc::Retained;
    use objc2::runtime::{Bool, ProtocolObject};
    use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    use objc2_foundation::{
        NSDictionary, NSError, NSObject, NSObjectProtocol, NSRunLoop, NSRunLoopCommonModes,
        NSString, NSTimer,
    };
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationPresentationOptions,
        UNNotificationRequest, UNNotificationResponse, UNNotificationSound,
        UNUserNotificationCenter, UNUserNotificationCenterDelegate,
    };

    const BROKER_EXECUTABLE_NAME: &str = "jcode-notification-broker";
    const POLL_INTERVAL_SECONDS: f64 = 0.25;
    const AUTHORIZATION_PENDING: u8 = 0;
    const AUTHORIZATION_GRANTED: u8 = 1;
    const AUTHORIZATION_DENIED: u8 = 2;
    const AUTHORIZATION_RETRY_TICKS: u32 = 240;
    const ORIGIN_METADATA_KEY: &str = "jcode_origin";

    define_class!(
        // SAFETY: NSObject has no subclassing requirements and BrokerDelegate
        // has no Drop implementation or Rust ivars.
        #[unsafe(super = NSObject)]
        #[thread_kind = MainThreadOnly]
        #[name = "JcodeNotificationBrokerDelegate"]
        struct BrokerDelegate;

        // SAFETY: NSObjectProtocol has no additional safety requirements.
        unsafe impl NSObjectProtocol for BrokerDelegate {}

        // SAFETY: Method signatures exactly match UNUserNotificationCenterDelegate.
        unsafe impl UNUserNotificationCenterDelegate for BrokerDelegate {
            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn will_present(
                &self,
                _center: &UNUserNotificationCenter,
                _notification: &objc2_user_notifications::UNNotification,
                completion_handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
            ) {
                completion_handler.call((UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List
                    | UNNotificationPresentationOptions::Sound,));
            }

            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn did_receive_response(
                &self,
                _center: &UNUserNotificationCenter,
                response: &UNNotificationResponse,
                completion_handler: &block2::DynBlock<dyn Fn()>,
            ) {
                let content = response.notification().request().content();
                let metadata = content.userInfo();
                // SAFETY: the broker creates this dictionary with NSString keys
                // and values. Notification Center property-list serialization
                // preserves those concrete types across process launches.
                let metadata = unsafe { metadata.cast_unchecked::<NSString, NSString>() };
                let key = NSString::from_str(ORIGIN_METADATA_KEY);
                let route = metadata
                    .objectForKey(&key)
                    .map(|value| value.to_string())
                    // Read the v1 target identifier too, so notifications posted
                    // by a broker upgraded in place remain clickable.
                    .or_else(|| {
                        content
                            .targetContentIdentifier()
                            .map(|value| value.to_string())
                    })
                    .and_then(|value| serde_json::from_str(&value).ok());
                if let Some(origin) = route {
                    crate::notifications::activate_macos_notification_origin(&origin);
                }
                completion_handler.call(());
            }
        }
    );

    impl BrokerDelegate {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(());
            // SAFETY: NSObject's `init` signature is correct for this subclass.
            unsafe { msg_send![super(this), init] }
        }
    }

    pub(super) fn is_invocation() -> bool {
        std::env::current_exe()
            .ok()
            .and_then(|path| path.file_name().map(|name| name.to_owned()))
            .is_some_and(|name| name == BROKER_EXECUTABLE_NAME)
    }

    pub(super) fn run() -> Result<()> {
        let mtm = MainThreadMarker::new()
            .context("macOS notification broker must start on the main thread")?;
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

        let center = UNUserNotificationCenter::currentNotificationCenter();
        let delegate = BrokerDelegate::new(mtm);
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

        let authorization = Arc::new(AtomicU8::new(AUTHORIZATION_PENDING));
        request_authorization(&center, authorization.clone());
        recover_interrupted_submissions();

        spawn_inbox_timer(center.clone(), authorization);
        // Keep both weakly-held delegates alive for the complete application run.
        app.run();
        drop(delegate);
        Ok(())
    }

    fn request_authorization(center: &UNUserNotificationCenter, authorization: Arc<AtomicU8>) {
        let callback = block2::RcBlock::new(move |granted: Bool, _error: *mut NSError| {
            authorization.store(
                if granted.as_bool() {
                    AUTHORIZATION_GRANTED
                } else {
                    AUTHORIZATION_DENIED
                },
                Ordering::Release,
            );
        });
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
            &callback,
        );
    }

    fn spawn_inbox_timer(center: Retained<UNUserNotificationCenter>, authorization: Arc<AtomicU8>) {
        let ticks = AtomicU32::new(0);
        let block = block2::RcBlock::new(move |_timer: NonNull<NSTimer>| {
            match authorization.load(Ordering::Acquire) {
                AUTHORIZATION_GRANTED => drain_inbox(&center),
                AUTHORIZATION_DENIED
                    if ticks.fetch_add(1, Ordering::Relaxed) % AUTHORIZATION_RETRY_TICKS == 0 =>
                {
                    // Permission may be enabled while the helper is running.
                    // Re-query without dropping queued work; macOS only presents
                    // the authorization prompt on the initial request.
                    request_authorization(&center, authorization.clone());
                }
                _ => {}
            }
        });
        unsafe {
            let timer =
                NSTimer::timerWithTimeInterval_repeats_block(POLL_INTERVAL_SECONDS, true, &block);
            NSRunLoop::currentRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes);
            // The run loop owns the timer, whose block owns the center.
            std::mem::forget(timer);
        }
    }

    fn recover_interrupted_submissions() {
        let Some(inbox) = crate::notifications::macos_notification_inbox_dir() else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(inbox) else {
            return;
        };
        for path in entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "submitting")
            })
        {
            let _ = std::fs::rename(&path, path.with_extension("json"));
        }
    }

    fn drain_inbox(center: &UNUserNotificationCenter) {
        let Some(inbox) = crate::notifications::macos_notification_inbox_dir() else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(inbox) else {
            return;
        };
        let mut paths: Vec<_> = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .collect();
        paths.sort();

        for path in paths {
            let result = std::fs::read(&path)
                .context("read queued notification")
                .and_then(|bytes| {
                    serde_json::from_slice::<crate::notifications::MacosNotificationEnvelope>(
                        &bytes,
                    )
                    .context("decode queued notification")
                })
                .and_then(|envelope| submit(center, &envelope, &path));
            match result {
                // The Notification Center completion handler owns removal or
                // retry after this point.
                Ok(()) => {}
                Err(error) => {
                    crate::logging::warn(&format!(
                        "macOS notification broker skipped {}: {error:#}",
                        path.display()
                    ));
                    // Quarantine poison payloads so one bad file cannot block the
                    // durable FIFO forever. A future schema is retained for an
                    // upgraded broker instead.
                    if !is_future_schema(&path) {
                        let _ = std::fs::rename(&path, path.with_extension("rejected"));
                    }
                }
            }
        }
    }

    fn is_future_schema(path: &std::path::Path) -> bool {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|value| value.get("schema_version")?.as_u64())
            .is_some_and(|version| {
                version > crate::notifications::MACOS_NOTIFICATION_SCHEMA_VERSION as u64
            })
    }

    fn submit(
        center: &UNUserNotificationCenter,
        envelope: &crate::notifications::MacosNotificationEnvelope,
        queued_path: &std::path::Path,
    ) -> Result<()> {
        anyhow::ensure!(
            envelope.schema_version == crate::notifications::MACOS_NOTIFICATION_SCHEMA_VERSION,
            "unsupported notification schema {}",
            envelope.schema_version
        );
        anyhow::ensure!(
            !envelope.notification_id.is_empty() && envelope.notification_id.len() <= 512,
            "invalid notification identifier"
        );

        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&envelope.title));
        if let Some(subtitle) = envelope.subtitle.as_deref() {
            content.setSubtitle(&NSString::from_str(subtitle));
        }
        content.setBody(&NSString::from_str(&envelope.body));
        content.setThreadIdentifier(&NSString::from_str("jcode-turn-complete"));
        let route = serde_json::to_string(&envelope.origin)?;
        let metadata_key = NSString::from_str(ORIGIN_METADATA_KEY);
        let metadata_value = NSString::from_str(&route);
        let metadata =
            NSDictionary::<NSString, NSString>::from_slices(&[&*metadata_key], &[&*metadata_value]);
        // SAFETY: NSString is a property-list type accepted by UserNotifications,
        // and both key and value remain retained by the immutable dictionary.
        unsafe { content.setUserInfo(metadata.cast_unchecked()) };
        if let Some(sound) = envelope.sound.as_deref() {
            let sound = UNNotificationSound::soundNamed(&NSString::from_str(sound));
            content.setSound(Some(&sound));
        }

        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(&envelope.notification_id),
            &content,
            None,
        );
        let submitting_path = queued_path.with_extension("submitting");
        std::fs::rename(queued_path, &submitting_path)
            .context("claim queued notification for submission")?;
        let retry_path = queued_path.to_path_buf();
        let completion = block2::RcBlock::new(move |error: *mut NSError| {
            if error.is_null() {
                let _ = std::fs::remove_file(&submitting_path);
            } else {
                // Submission errors are generally transient (authorization or
                // Notification Center availability). Preserve the payload for
                // the next timer pass or helper launch.
                let _ = std::fs::rename(&submitting_path, &retry_path);
                crate::logging::warn("macOS Notification Center rejected a queued notification");
            }
        });
        center.addNotificationRequest_withCompletionHandler(&request, Some(&completion));
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub fn is_invocation() -> bool {
    platform::is_invocation()
}

#[cfg(not(target_os = "macos"))]
pub fn is_invocation() -> bool {
    false
}

#[cfg(target_os = "macos")]
pub fn run() -> anyhow::Result<()> {
    platform::run()
}

#[cfg(not(target_os = "macos"))]
pub fn run() -> anyhow::Result<()> {
    anyhow::bail!("the notification broker is only available on macOS")
}
