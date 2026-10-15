//! Process-level isolation for shell commands run by the bash tool.
//!
//! Two backends:
//! - `local` (default): the command runs on the host, but on Windows the
//!   whole process tree is placed in a Job Object so timeouts kill every
//!   descendant and optional process/memory limits are enforced by the OS.
//! - `docker`: the command runs in a throwaway container with only the
//!   workspace mounted and, unless explicitly allowed, no network at all.
//!   This is real filesystem + network isolation, independent of how the
//!   command is spelled.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecBackend {
    Local,
    Docker(DockerConfig),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerConfig {
    pub image: String,
    pub allow_network: bool,
    pub memory: Option<String>,
    pub cpus: Option<String>,
}

pub const DEFAULT_DOCKER_IMAGE: &str = "debian:bookworm-slim";

fn env_flag(key: &str) -> bool {
    matches!(
        env::var(key).unwrap_or_default().trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "on" | "yes"
    )
}

fn env_nonempty(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

impl ExecBackend {
    pub fn from_env() -> Self {
        let mode = env::var("ASI_SANDBOX").unwrap_or_default();
        if mode.trim().eq_ignore_ascii_case("docker") {
            Self::Docker(DockerConfig {
                image: env_nonempty("ASI_SANDBOX_DOCKER_IMAGE")
                    .unwrap_or_else(|| DEFAULT_DOCKER_IMAGE.to_string()),
                allow_network: env_flag("ASI_SANDBOX_ALLOW_NETWORK"),
                memory: env_nonempty("ASI_SANDBOX_DOCKER_MEMORY"),
                cpus: env_nonempty("ASI_SANDBOX_DOCKER_CPUS"),
            })
        } else {
            Self::Local
        }
    }
}

/// Convert a host path to a form `docker -v` accepts on every platform
/// (forward slashes; Windows drive letters are kept, e.g. `D:/proj`).
fn docker_host_path(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    s.strip_prefix("//?/").map(str::to_string).unwrap_or(s)
}

/// Build the `docker run` invocation for `command`. The workspace is
/// mounted read-write at /workspace and is the only host directory visible.
pub fn docker_command(cfg: &DockerConfig, workspace: &Path, command: &str) -> Command {
    let mut c = Command::new("docker");
    c.args(docker_args(cfg, workspace, command));
    c
}

pub fn docker_args(cfg: &DockerConfig, workspace: &Path, command: &str) -> Vec<String> {
    let mut args = vec![
        "run".to_string(),
        "--rm".to_string(),
        "-i".to_string(),
        "--security-opt".to_string(),
        "no-new-privileges".to_string(),
        "--cap-drop".to_string(),
        "ALL".to_string(),
        "--pids-limit".to_string(),
        "512".to_string(),
    ];
    if !cfg.allow_network {
        args.push("--network".to_string());
        args.push("none".to_string());
    }
    if let Some(m) = &cfg.memory {
        args.push("--memory".to_string());
        args.push(m.clone());
    }
    if let Some(c) = &cfg.cpus {
        args.push("--cpus".to_string());
        args.push(c.clone());
    }
    args.push("-v".to_string());
    args.push(format!("{}:/workspace", docker_host_path(workspace)));
    args.push("-w".to_string());
    args.push("/workspace".to_string());
    args.push(cfg.image.clone());
    args.push("sh".to_string());
    args.push("-lc".to_string());
    args.push(command.to_string());
    args
}

pub fn current_workspace() -> PathBuf {
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

pub fn keep_background_processes() -> bool {
    env_flag("ASI_SANDBOX_KEEP_BACKGROUND")
}

/// Limits applied to the local process tree (Windows Job Object).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProcessLimits {
    pub max_processes: Option<u32>,
    pub max_memory_bytes: Option<usize>,
}

impl ProcessLimits {
    pub fn from_env() -> Self {
        Self {
            max_processes: env_nonempty("ASI_SANDBOX_MAX_PROCESSES")
                .and_then(|v| v.parse::<u32>().ok())
                .filter(|v| *v > 0),
            max_memory_bytes: env_nonempty("ASI_SANDBOX_MAX_MEMORY_MB")
                .and_then(|v| v.parse::<usize>().ok())
                .filter(|v| *v > 0)
                .map(|mb| mb * 1024 * 1024),
        }
    }
}

#[cfg(windows)]
mod job {
    use super::ProcessLimits;
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_JOB_MEMORY,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    /// Owns a Job Object. Dropping it closes the handle, and because the job
    /// is created with KILL_ON_JOB_CLOSE every process still inside dies.
    #[derive(Debug)]
    pub struct ProcessTreeGuard {
        handle: HANDLE,
    }

    // The raw handle is only used through kernel calls that are thread-safe.
    unsafe impl Send for ProcessTreeGuard {}

    impl ProcessTreeGuard {
        pub fn attach(child: &Child, limits: ProcessLimits) -> Result<Self, String> {
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return Err(format!("CreateJobObjectW failed: {}", std::io::Error::last_os_error()));
                }
                let guard = Self { handle };
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if let Some(n) = limits.max_processes {
                    info.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
                    info.BasicLimitInformation.ActiveProcessLimit = n;
                }
                if let Some(bytes) = limits.max_memory_bytes {
                    info.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_JOB_MEMORY;
                    info.JobMemoryLimit = bytes;
                }
                let ok = SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    return Err(format!("SetInformationJobObject failed: {}", std::io::Error::last_os_error()));
                }
                if AssignProcessToJobObject(handle, child.as_raw_handle() as HANDLE) == 0 {
                    return Err(format!("AssignProcessToJobObject failed: {}", std::io::Error::last_os_error()));
                }
                Ok(guard)
            }
        }

        /// Kill every process in the tree (used on timeout).
        pub fn terminate(&self) {
            unsafe {
                TerminateJobObject(self.handle, 1);
            }
        }
    }

    impl Drop for ProcessTreeGuard {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }
}

#[cfg(not(windows))]
mod job {
    use super::ProcessLimits;
    use std::process::Child;

    /// Non-Windows placeholder: process-tree containment is provided by the
    /// docker backend there.
    #[derive(Debug)]
    pub struct ProcessTreeGuard;

    impl ProcessTreeGuard {
        pub fn attach(_child: &Child, _limits: ProcessLimits) -> Result<Self, String> {
            Ok(Self)
        }
        pub fn terminate(&self) {}
    }
}

pub use job::ProcessTreeGuard;

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(allow_network: bool) -> DockerConfig {
        DockerConfig {
            image: "alpine:3".to_string(),
            allow_network,
            memory: Some("512m".to_string()),
            cpus: None,
        }
    }

    #[test]
    fn docker_args_disable_network_by_default_and_mount_only_workspace() {
        let args = docker_args(&cfg(false), Path::new(r"D:\proj\app"), "cargo test");
        let joined = args.join(" ");
        assert!(joined.contains("--network none"), "{}", joined);
        assert!(joined.contains("-v D:/proj/app:/workspace"), "{}", joined);
        assert!(joined.contains("--cap-drop ALL"));
        assert!(joined.contains("--memory 512m"));
        assert_eq!(args.iter().filter(|a| a.as_str() == "-v").count(), 1);
        assert_eq!(args.last().unwrap(), "cargo test");
    }

    #[test]
    fn docker_args_allow_network_when_opted_in() {
        let args = docker_args(&cfg(true), Path::new("/tmp/p"), "true");
        assert!(!args.iter().any(|a| a == "none"));
    }

    #[test]
    fn docker_host_path_strips_verbatim_prefix() {
        assert_eq!(docker_host_path(Path::new(r"\\?\D:\x")), "D:/x");
    }

    #[cfg(windows)]
    #[test]
    fn job_object_kills_grandchild_processes_on_terminate() {
        use std::process::Stdio;
        use std::time::{Duration, Instant};
        // powershell spawns a long-running grandchild; terminating the job
        // must end the whole tree promptly.
        let mut child = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "Start-Process -NoNewWindow -Wait powershell -ArgumentList '-NoProfile','-Command','Start-Sleep 60'",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn powershell");
        let guard = ProcessTreeGuard::attach(&child, ProcessLimits::default()).expect("attach job");
        std::thread::sleep(Duration::from_millis(1500));
        let started = Instant::now();
        guard.terminate();
        let _ = child.wait();
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[cfg(windows)]
    #[test]
    fn job_object_enforces_process_limit() {
        use std::process::Stdio;
        let mut child = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "Start-Sleep -Milliseconds 800; try { Start-Process -NoNewWindow -Wait cmd -ArgumentList '/c','exit 0'; 'spawned' } catch { 'blocked' }",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn powershell");
        let _guard = ProcessTreeGuard::attach(
            &child,
            ProcessLimits {
                max_processes: Some(1),
                max_memory_bytes: None,
            },
        )
        .expect("attach job");
        let out = child.wait_with_output_compat();
        assert!(out.contains("blocked"), "child spawn should be denied, got: {}", out);
    }

    #[cfg(windows)]
    trait WaitCompat {
        fn wait_with_output_compat(&mut self) -> String;
    }

    #[cfg(windows)]
    impl WaitCompat for std::process::Child {
        fn wait_with_output_compat(&mut self) -> String {
            use std::io::Read;
            let mut s = String::new();
            if let Some(mut out) = self.stdout.take() {
                let _ = out.read_to_string(&mut s);
            }
            let _ = self.wait();
            s
        }
    }
}
