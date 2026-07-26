use std::{path::PathBuf, process::ExitCode};

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
    let mut arguments = std::env::args_os().skip(1);
    let Some(first) = arguments.next() else {
        return Err(usage());
    };
    if first == "--version" {
        println!(
            "easytier-hev-socks-egress {} {}",
            easytier_socks_egress::managed_backend_name(),
            easytier_socks_egress::managed_backend_revision()
        );
        return Ok(());
    }
    let config_path = PathBuf::from(first);
    let workers_flag = arguments
        .next()
        .ok_or_else(usage)?
        .into_string()
        .map_err(|_| usage())?;
    if workers_flag != "--workers" {
        return Err(usage());
    }
    let workers = arguments
        .next()
        .ok_or_else(usage)?
        .into_string()
        .map_err(|_| usage())?
        .parse::<usize>()
        .map_err(|_| "invalid --workers value".to_owned())?;

    #[cfg(target_os = "macos")]
    {
        let flag = arguments
            .next()
            .ok_or_else(usage)?
            .into_string()
            .map_err(|_| usage())?;
        if flag != "--parent-pid" {
            return Err(usage());
        }
        let parent_pid = arguments
            .next()
            .ok_or_else(usage)?
            .into_string()
            .map_err(|_| usage())?
            .parse::<libc::pid_t>()
            .map_err(|_| "invalid --parent-pid value".to_owned())?;
        start_parent_watchdog(parent_pid)?;
    }

    if arguments.next().is_some() {
        return Err(usage());
    }
    easytier_socks_egress::run_managed_server_from_file(&config_path, workers)
}

fn usage() -> String {
    #[cfg(target_os = "macos")]
    return "usage: easytier-hev-socks-egress CONFIG_PATH --workers N --parent-pid PID".to_owned();
    #[cfg(not(target_os = "macos"))]
    return "usage: easytier-hev-socks-egress CONFIG_PATH --workers N".to_owned();
}

#[cfg(target_os = "macos")]
fn start_parent_watchdog(parent_pid: libc::pid_t) -> Result<(), String> {
    if parent_pid <= 1 || unsafe { libc::getppid() } != parent_pid {
        return Err("EasyTier parent exited while starting HEV".to_owned());
    }
    std::thread::Builder::new()
        .name("easytier-hev-parent-watch".to_owned())
        .spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_millis(250));
                if unsafe { libc::getppid() } != parent_pid {
                    std::process::exit(0);
                }
            }
        })
        .map(|_| ())
        .map_err(|error| format!("failed to start HEV parent watchdog: {error}"))
}
