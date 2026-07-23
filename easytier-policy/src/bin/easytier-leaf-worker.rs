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
            "--parent-pid" => {
                parent_pid = Some(
                    args.next()
                        .ok_or_else(|| "missing --parent-pid value".to_owned())?
                        .to_string_lossy()
                        .parse::<u32>()
                        .map_err(|_| "invalid --parent-pid value".to_owned())?,
                );
            }
            other => return Err(format!("unknown worker argument: {other}")),
        }
    }
    let config = config.ok_or_else(|| "missing -c policy config".to_owned())?;
    let outbound_interface = outbound_interface
        .as_deref()
        .map(validate_outbound_interface)
        .transpose()?;
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
    leaf::start(
        0,
        leaf::StartOptions {
            config: leaf::Config::Str(config),
            runtime_opt,
        },
    )
    .map_err(|error| error.to_string())
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
fn start_parent_watchdog(parent_pid: u32) -> Result<(), String> {
    if parent_pid <= 1 || unsafe { libc::getppid() } as u32 != parent_pid {
        return Err("EasyTier parent exited while starting Leaf".to_owned());
    }
    std::thread::Builder::new()
        .name("easytier-leaf-parent-watch".to_owned())
        .spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                if unsafe { libc::getppid() } as u32 != parent_pid {
                    std::process::exit(0);
                }
            }
        })
        .map(|_| ())
        .map_err(|error| format!("failed to start Leaf parent watchdog: {error}"))
}

#[cfg(target_os = "windows")]
fn start_parent_watchdog(parent_pid: u32) -> Result<(), String> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, WAIT_OBJECT_0},
        System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    };

    let parent = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, parent_pid) };
    if parent.is_null() {
        return Err("EasyTier parent exited while starting Leaf".to_owned());
    }
    // A Windows HANDLE is represented as a raw pointer and is therefore not
    // `Send`. Transfer its integer value and reconstruct it in the watchdog.
    let parent_handle = parent as usize;
    let watcher = std::thread::Builder::new()
        .name("easytier-leaf-parent-watch".to_owned())
        .spawn(move || {
            let parent = parent_handle as windows_sys::Win32::Foundation::HANDLE;
            let result = unsafe { WaitForSingleObject(parent, u32::MAX) };
            unsafe { CloseHandle(parent) };
            if result == WAIT_OBJECT_0 {
                std::process::exit(0);
            }
        });
    if let Err(error) = watcher {
        unsafe { CloseHandle(parent_handle as windows_sys::Win32::Foundation::HANDLE) };
        return Err(format!("failed to start Leaf parent watchdog: {error}"));
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn start_parent_watchdog(_parent_pid: u32) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "linux")]
fn validate_outbound_interface(interface: &str) -> Result<String, String> {
    use std::ffi::CString;

    let interface_name = interface.to_owned();
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
    Ok(interface_name)
}

#[cfg(target_os = "macos")]
fn validate_outbound_interface(interface: &str) -> Result<String, String> {
    use std::ffi::CString;

    let interface_name = interface.to_owned();
    let interface =
        CString::new(interface).map_err(|_| "outbound interface contains a NUL byte".to_owned())?;
    if unsafe { libc::if_nametoindex(interface.as_ptr()) } == 0 {
        return Err(format!(
            "policy outbound interface does not exist: {}",
            interface.to_string_lossy()
        ));
    }
    Ok(interface_name)
}

#[cfg(target_os = "windows")]
fn validate_outbound_interface(interface: &str) -> Result<String, String> {
    easytier_policy::windows_underlay(interface).map(|underlay| underlay.interface_name)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn validate_outbound_interface(interface: &str) -> Result<String, String> {
    Ok(interface.to_owned())
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
