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
            "--single-thread" => {}
            other => return Err(format!("unknown worker argument: {other}")),
        }
    }
    let config = config.ok_or_else(|| "missing -c policy config".to_owned())?;
    if check {
        return leaf::test_config(&config).map_err(|error| error.to_string());
    }
    if let Some(interface) = outbound_interface {
        // The worker is a dedicated process; this cannot affect EasyTier's environment.
        unsafe { std::env::set_var("OUTBOUND_INTERFACE", interface) };
    }
    leaf::start(
        0,
        leaf::StartOptions {
            config: leaf::Config::File(config),
            runtime_opt: leaf::RuntimeOption::SingleThread,
        },
    )
    .map_err(|error| error.to_string())
}
