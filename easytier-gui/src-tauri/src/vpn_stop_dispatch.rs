pub(crate) fn dispatch<NativeStop, FrontendFallback>(
    has_tun: bool,
    native_stop_is_authoritative: bool,
    stop_native: NativeStop,
    notify_frontend: FrontendFallback,
) -> Result<(), String>
where
    NativeStop: FnOnce() -> Result<(), String>,
    FrontendFallback: FnOnce() -> Result<(), String>,
{
    if has_tun {
        return Ok(());
    }

    if !native_stop_is_authoritative {
        return notify_frontend();
    }

    // Clash Meta's TunService closes its TUN in the native service's
    // NonCancellable finally block before stopSelf(). Android VPN FD ownership
    // must likewise not depend on EasyTier's WebView queue. A successful native
    // stop is final; enqueue the legacy WebView notification only after failure.
    match stop_native() {
        Ok(()) => Ok(()),
        Err(native_error) => {
            let _ = notify_frontend();
            Err(native_error)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::dispatch;

    #[test]
    fn does_nothing_while_a_tun_instance_remains() {
        let calls = RefCell::new(Vec::new());
        dispatch(
            true,
            true,
            || {
                calls.borrow_mut().push("native");
                Ok::<(), String>(())
            },
            || {
                calls.borrow_mut().push("frontend");
                Ok::<(), String>(())
            },
        )
        .unwrap();

        assert!(calls.borrow().is_empty());
    }

    #[test]
    fn native_success_is_final() {
        let calls = RefCell::new(Vec::new());
        dispatch(
            false,
            true,
            || {
                calls.borrow_mut().push("native");
                Ok(())
            },
            || {
                calls.borrow_mut().push("frontend");
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(*calls.borrow(), ["native"]);
    }

    #[test]
    fn native_failure_keeps_frontend_fallback_and_error() {
        let calls = RefCell::new(Vec::new());
        let error = dispatch(
            false,
            true,
            || {
                calls.borrow_mut().push("native");
                Err("native stop failed".to_string())
            },
            || {
                calls.borrow_mut().push("frontend");
                Ok(())
            },
        )
        .unwrap_err();

        assert_eq!(error, "native stop failed");
        assert_eq!(*calls.borrow(), ["native", "frontend"]);
    }

    #[test]
    fn frontend_remains_owner_without_native_support() {
        let calls = RefCell::new(Vec::new());
        dispatch(
            false,
            false,
            || {
                calls.borrow_mut().push("native");
                Ok(())
            },
            || {
                calls.borrow_mut().push("frontend");
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(*calls.borrow(), ["frontend"]);
    }
}
