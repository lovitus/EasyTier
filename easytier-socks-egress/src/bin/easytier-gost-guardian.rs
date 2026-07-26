use std::process::ExitCode;

#[cfg(any(target_os = "macos", target_os = "freebsd"))]
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::FAILURE,
    }
}

#[cfg(not(any(target_os = "macos", target_os = "freebsd")))]
fn main() -> ExitCode {
    ExitCode::FAILURE
}

#[cfg(any(target_os = "macos", target_os = "freebsd"))]
fn run() -> Result<(), ()> {
    let mut arguments = std::env::args_os().skip(1);
    let parent_pid = parse_pid(arguments.next()).ok_or(())?;
    let gost_pid = parse_pid(arguments.next()).ok_or(())?;
    if arguments.next().is_some() || parent_pid <= 1 || gost_pid <= 1 {
        return Err(());
    }

    let result = monitor(parent_pid, gost_pid);
    if result.is_err() {
        terminate(gost_pid);
    }
    result
}

#[cfg(any(target_os = "macos", target_os = "freebsd"))]
fn parse_pid(value: Option<std::ffi::OsString>) -> Option<libc::pid_t> {
    value?.to_str()?.parse().ok()
}

#[cfg(any(target_os = "macos", target_os = "freebsd"))]
fn monitor(parent_pid: libc::pid_t, gost_pid: libc::pid_t) -> Result<(), ()> {
    // SAFETY: kqueue returns a new descriptor owned by this process.
    let queue = unsafe { libc::kqueue() };
    if queue < 0 {
        return Err(());
    }
    let _queue = Queue(queue);

    let mut changes = [process_exit_event(parent_pid), process_exit_event(gost_pid)];
    // SAFETY: both slices point to initialized kevent storage for the duration
    // of the call, and queue is an open kqueue descriptor.
    if unsafe {
        libc::kevent(
            queue,
            changes.as_mut_ptr(),
            changes.len() as i32,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
        )
    } < 0
    {
        return Err(());
    }

    let mut events = [
        std::mem::MaybeUninit::<libc::kevent>::uninit(),
        std::mem::MaybeUninit::<libc::kevent>::uninit(),
    ];
    // SAFETY: kevent initializes `ready` events before returning a positive
    // count.
    let ready = unsafe {
        libc::kevent(
            queue,
            std::ptr::null(),
            0,
            events.as_mut_ptr().cast(),
            events.len() as i32,
            std::ptr::null(),
        )
    };
    if ready <= 0 {
        return Err(());
    }
    // Prefer the child's exit when both events arrive together. This prevents
    // sending a signal to a PID that has already exited.
    let ready_events = &events[..ready as usize];
    if ready_events
        .iter()
        // SAFETY: kevent initialized every element before `ready`.
        .any(|event| unsafe { event.assume_init_ref().ident == gost_pid as usize })
    {
        return Ok(());
    }
    if ready_events
        .iter()
        // SAFETY: kevent initialized every element before `ready`.
        .any(|event| unsafe { event.assume_init_ref().ident == parent_pid as usize })
    {
        terminate(gost_pid);
    }
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "freebsd"))]
fn process_exit_event(pid: libc::pid_t) -> libc::kevent {
    // SAFETY: zero is a valid initial state and every field consumed by
    // kevent is assigned below.
    let mut event = unsafe { std::mem::zeroed::<libc::kevent>() };
    event.ident = pid as usize;
    event.filter = libc::EVFILT_PROC;
    event.flags = libc::EV_ADD | libc::EV_ENABLE | libc::EV_ONESHOT;
    event.fflags = libc::NOTE_EXIT;
    event
}

#[cfg(any(target_os = "macos", target_os = "freebsd"))]
fn terminate(pid: libc::pid_t) {
    // SAFETY: signals are sent only to the exact child PID supplied by the
    // parent process.
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
    for _ in 0..60 {
        // SAFETY: signal 0 only probes whether the exact PID still exists.
        if unsafe { libc::kill(pid, 0) } != 0 {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // SAFETY: the bounded graceful shutdown expired for the exact child PID.
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
}

#[cfg(any(target_os = "macos", target_os = "freebsd"))]
struct Queue(libc::c_int);

#[cfg(any(target_os = "macos", target_os = "freebsd"))]
impl Drop for Queue {
    fn drop(&mut self) {
        // SAFETY: Queue exclusively owns this descriptor.
        unsafe {
            libc::close(self.0);
        }
    }
}
