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
    if let Some(interface) = outbound_interface {
        // The worker is a dedicated process; this cannot affect EasyTier's environment.
        unsafe { std::env::set_var("OUTBOUND_INTERFACE", interface) };
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
    leaf::start(
        0,
        leaf::StartOptions {
            config: leaf::Config::File(config),
            runtime_opt,
        },
    )
    .map_err(|error| error.to_string())
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

#[cfg(not(target_os = "linux"))]
fn validate_outbound_interface(_interface: &str) -> Result<(), String> {
    Ok(())
}
