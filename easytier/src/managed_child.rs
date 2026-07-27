use std::{
    path::{Path, PathBuf},
    time::Duration,
};

#[cfg(any(windows, target_os = "macos", target_os = "freebsd"))]
use anyhow::Context as _;
#[cfg(any(target_os = "macos", target_os = "freebsd"))]
use anyhow::bail;
#[cfg(any(target_os = "macos", target_os = "freebsd"))]
use std::process::Stdio;
use tokio::process::{Child, Command};

fn guardian_executable_candidates(
    current_executable: &Path,
    managed_executable: &Path,
) -> Vec<PathBuf> {
    let guardian_name = format!("easytier-gost-guardian{}", std::env::consts::EXE_SUFFIX);
    let primary = current_executable.with_file_name(&guardian_name);
    let fallback = managed_executable.with_file_name(guardian_name);
    if primary == fallback {
        vec![primary]
    } else {
        vec![primary, fallback]
    }
}

pub(crate) fn configure_command(command: &mut Command) {
    command.kill_on_drop(true);

    #[cfg(target_os = "linux")]
    {
        let expected_parent = std::process::id() as nix::libc::pid_t;
        // SAFETY: prctl only changes the child process death signal between
        // fork and exec. The parent check closes the race where Core exits
        // between fork and PR_SET_PDEATHSIG.
        unsafe {
            command.pre_exec(move || {
                if nix::libc::prctl(nix::libc::PR_SET_PDEATHSIG, nix::libc::SIGTERM) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if nix::libc::getppid() != expected_parent {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "EasyTier parent exited before sidecar exec",
                    ));
                }
                Ok(())
            });
        }
    }

    #[cfg(windows)]
    {
        command.creation_flags(0x0800_0000);
    }
}

pub(crate) struct ManagedChild {
    child: Child,
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    guardian: Child,
    #[cfg(windows)]
    _job: WindowsJob,
}

impl ManagedChild {
    pub(crate) async fn attach(child: Child, _executable: &Path) -> anyhow::Result<Self> {
        #[cfg(any(target_os = "macos", target_os = "freebsd"))]
        {
            let mut child = child;
            let child_pid = child
                .id()
                .context("managed sidecar exited before guardian attachment")?;
            let current_executable =
                std::env::current_exe().context("failed to locate the EasyTier executable")?;
            let guardian_candidates =
                guardian_executable_candidates(&current_executable, _executable);
            let Some(guardian_executable) = guardian_candidates
                .iter()
                .find(|candidate| candidate.is_file())
            else {
                let _ = child.start_kill();
                let _ = child.wait().await;
                bail!(
                    "managed sidecar guardian is missing; tried {}",
                    guardian_candidates
                        .iter()
                        .map(|candidate| candidate.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            };
            let mut guardian = Command::new(guardian_executable);
            guardian
                .arg(std::process::id().to_string())
                .arg(child_pid.to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            configure_command(&mut guardian);
            let mut guardian = match guardian.spawn() {
                Ok(guardian) => guardian,
                Err(error) => {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    return Err(error).with_context(|| {
                        format!(
                            "failed to start managed sidecar guardian {}",
                            guardian_executable.display()
                        )
                    });
                }
            };
            tokio::time::sleep(Duration::from_millis(25)).await;
            if let Some(status) = guardian
                .try_wait()
                .context("failed to inspect managed sidecar guardian")?
            {
                let _ = child.start_kill();
                let _ = child.wait().await;
                bail!("managed sidecar guardian exited during attachment with {status}");
            }
            return Ok(Self { child, guardian });
        }

        #[cfg(windows)]
        {
            let mut child = child;
            let job = match WindowsJob::attach(&child) {
                Ok(job) => job,
                Err(error) => {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    return Err(error);
                }
            };
            return Ok(Self { child, _job: job });
        }

        #[cfg(not(any(windows, target_os = "macos", target_os = "freebsd")))]
        Ok(Self { child })
    }

    pub(crate) fn id(&self) -> Option<u32> {
        self.child.id()
    }

    pub(crate) fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.child.try_wait()
    }

    pub(crate) fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    pub(crate) fn start_kill(&mut self) {
        #[cfg(any(target_os = "macos", target_os = "freebsd"))]
        {
            let _ = self.guardian.start_kill();
        }
        let _ = self.child.start_kill();
    }

    pub(crate) async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        let status = self.child.wait().await;
        self.reap_guardian().await;
        status
    }

    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    async fn reap_guardian(&mut self) {
        if tokio::time::timeout(Duration::from_secs(1), self.guardian.wait())
            .await
            .is_err()
        {
            let _ = self.guardian.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(1), self.guardian.wait()).await;
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "freebsd")))]
    async fn reap_guardian(&mut self) {}

    pub(crate) async fn terminate(&mut self, timeout: Duration) {
        self.start_kill();
        if tokio::time::timeout(timeout, self.child.wait())
            .await
            .is_err()
        {
            let _ = self.child.kill().await;
        }
        self.reap_guardian().await;
    }
}

#[cfg(windows)]
struct WindowsJob {
    _handle: std::os::windows::io::OwnedHandle,
}

#[cfg(windows)]
impl WindowsJob {
    fn attach(child: &Child) -> anyhow::Result<Self> {
        use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _};
        use windows::{
            Win32::{
                Foundation::{CloseHandle, HANDLE},
                System::{
                    JobObjects::{
                        AssignProcessToJobObject, CreateJobObjectW,
                        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                        JobObjectExtendedLimitInformation, SetInformationJobObject,
                    },
                    Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE},
                },
            },
            core::PCWSTR,
        };

        let child_pid = child
            .id()
            .context("managed sidecar exited before Windows job attachment")?;
        // SAFETY: the created handles are owned locally, the information
        // structure remains valid for the call, and the process handle is
        // closed immediately after assignment.
        unsafe {
            let job = CreateJobObjectW(None, PCWSTR::null())
                .context("failed to create managed sidecar Windows job")?;
            let job = std::os::windows::io::OwnedHandle::from_raw_handle(job.0);
            let job_handle = HANDLE(job.as_raw_handle());
            let mut information = std::mem::zeroed::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>();
            information.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if let Err(error) = SetInformationJobObject(
                job_handle,
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of!(information).cast(),
                std::mem::size_of_val(&information) as u32,
            ) {
                return Err(error).context("failed to configure managed sidecar Windows job");
            }
            let process = match OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, child_pid)
            {
                Ok(process) => process,
                Err(error) => {
                    return Err(error).context("failed to open managed sidecar process");
                }
            };
            let assignment = AssignProcessToJobObject(job_handle, process);
            let _ = CloseHandle(process);
            if let Err(error) = assignment {
                return Err(error).context("failed to assign managed sidecar to Windows job");
            }
            Ok(Self { _handle: job })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::guardian_executable_candidates;
    use std::path::{Path, PathBuf};

    #[test]
    fn guardian_prefers_the_easytier_directory_then_the_managed_executable_directory() {
        let candidates = guardian_executable_candidates(
            Path::new("/opt/easytier/easytier-core"),
            Path::new("/custom/proxy/mihomo"),
        );

        assert_eq!(
            candidates,
            vec![
                PathBuf::from("/opt/easytier/easytier-gost-guardian"),
                PathBuf::from("/custom/proxy/easytier-gost-guardian"),
            ]
        );
    }

    #[test]
    fn guardian_candidate_is_deduplicated_for_the_same_directory() {
        let candidates = guardian_executable_candidates(
            Path::new("/opt/easytier/easytier-core"),
            Path::new("/opt/easytier/mihomo"),
        );

        assert_eq!(
            candidates,
            vec![PathBuf::from("/opt/easytier/easytier-gost-guardian")]
        );
    }

    #[cfg(windows)]
    #[test]
    fn managed_child_can_cross_tokio_worker_threads() {
        fn assert_send<T: Send>() {}
        assert_send::<super::ManagedChild>();
    }
}
