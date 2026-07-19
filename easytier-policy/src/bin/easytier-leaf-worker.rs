use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let mut config = None;
    let mut check = false;
    let mut outbound_interface = None;
    let mut single_thread = false;
    let mut parent_pid = None;
    let mut packet_batch = false;
    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "-c" => {
                config = args
                    .next()
                    .map(|value| value.to_string_lossy().into_owned());
            }
            "-T" => check = true,
            "-b" => {
                outbound_interface = args
                    .next()
                    .map(|value| value.to_string_lossy().into_owned());
            }
            "--single-thread" => single_thread = true,
            "--packet-batch" => packet_batch = true,
            "--parent-pid" => {
                parent_pid = Some(
                    args.next()
                        .ok_or_else(|| "missing --parent-pid value".to_owned())?
                        .to_string_lossy()
                        .parse::<libc::pid_t>()
                        .map_err(|_| "invalid --parent-pid value".to_owned())?,
                );
            }
            other => return Err(format!("unknown worker argument: {other}")),
        }
    }
    let config = config.ok_or_else(|| "missing -c policy config".to_owned())?;
    if let Some(interface) = outbound_interface.as_deref() {
        validate_outbound_interface(interface)?;
    }
    if check {
        return leaf::test_config(&config).map_err(|error| error.to_string());
    }
    let config = take_runtime_config(&config)?;
    if let Some(interface) = outbound_interface {
        // The worker is a dedicated process; this cannot affect EasyTier's environment.
        unsafe { std::env::set_var("OUTBOUND_INTERFACE", interface) };
    }
    if let Some(parent_pid) = parent_pid {
        start_parent_watchdog(parent_pid)?;
    }
    let runtime_opt = if single_thread {
        leaf::RuntimeOption::SingleThread
    } else {
        let workers = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
            .min(4);
        if workers == 1 {
            leaf::RuntimeOption::SingleThread
        } else {
            leaf::RuntimeOption::MultiThread(workers, 2 * 1024 * 1024)
        }
    };
    let start_options = leaf::StartOptions {
        config: leaf::Config::Str(config),
        runtime_opt,
    };
    let result = if packet_batch {
        let endpoint = unsafe { easytier_policy::LeafPacketStreamEndpoint::from_raw_fd(3) }
            .into_external_packet_endpoint()
            .map_err(|error| error.to_string())?;
        leaf::start_with_external_packet_endpoint(0, start_options, endpoint)
    } else {
        leaf::start(0, start_options)
    };
    result.map_err(|error| error.to_string())
}

fn take_runtime_config(path: &str) -> Result<String, String> {
    let config = std::fs::read_to_string(path)
        .map_err(|error| format!("failed to read private Leaf config: {error}"))?;
    // Leaf Config::Str preserves the same parser semantics while ensuring proxy
    // credentials are not left in a named file if the parent is killed.
    std::fs::remove_file(path)
        .map_err(|error| format!("failed to unlink private Leaf config: {error}"))?;
    Ok(config)
}

#[cfg(target_os = "macos")]
fn start_parent_watchdog(parent_pid: libc::pid_t) -> Result<(), String> {
    if parent_pid <= 1 || unsafe { libc::getppid() } != parent_pid {
        return Err("EasyTier parent exited while starting Leaf".to_owned());
    }
    std::thread::Builder::new()
        .name("easytier-leaf-parent-watch".to_owned())
        .spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                if unsafe { libc::getppid() } != parent_pid {
                    std::process::exit(0);
                }
            }
        })
        .map(|_| ())
        .map_err(|error| format!("failed to start Leaf parent watchdog: {error}"))
}

#[cfg(not(target_os = "macos"))]
fn start_parent_watchdog(_parent_pid: libc::pid_t) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "linux")]
fn validate_outbound_interface(interface: &str) -> Result<(), String> {
    use std::ffi::CString;

    let interface =
        CString::new(interface).map_err(|_| "outbound interface contains a NUL byte".to_owned())?;
    let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(format!(
            "failed to create outbound-interface probe socket: {}",
            std::io::Error::last_os_error()
        ));
    }
    let result = unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            interface.as_ptr().cast(),
            interface.as_bytes().len() as libc::socklen_t,
        )
    };
    let error = std::io::Error::last_os_error();
    unsafe { libc::close(fd) };
    if result != 0 {
        return Err(format!("cannot bind policy sockets to interface: {error}"));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn validate_outbound_interface(interface: &str) -> Result<(), String> {
    use std::ffi::CString;

    let interface =
        CString::new(interface).map_err(|_| "outbound interface contains a NUL byte".to_owned())?;
    if unsafe { libc::if_nametoindex(interface.as_ptr()) } == 0 {
        return Err(format!(
            "policy outbound interface does not exist: {}",
            interface.to_string_lossy()
        ));
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn validate_outbound_interface(_interface: &str) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn runtime_config_is_unlinked_after_read() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("leaf.json");
        std::fs::write(&path, "{\"log\":{\"level\":\"warn\"}}").unwrap();

        let config = super::take_runtime_config(path.to_str().unwrap()).unwrap();

        assert_eq!(config, "{\"log\":{\"level\":\"warn\"}}");
        assert!(!path.exists());
    }
}
