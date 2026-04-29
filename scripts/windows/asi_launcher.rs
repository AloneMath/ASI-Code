#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[cfg(target_os = "windows")]
mod win {
    use std::env;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn quote_cmd(input: &str) -> String {
        format!("\"{}\"", input.replace('"', "\"\""))
    }

    fn quoted_path(path: &Path) -> String {
        quote_cmd(&path.display().to_string())
    }

    fn backend_command(base_dir: &Path) -> String {
        let backend = base_dir.join("bin").join("asi.exe");
        if backend.exists() {
            let mut cmd = quoted_path(&backend);
            for arg in env::args().skip(1) {
                cmd.push(' ');
                cmd.push_str(&quote_cmd(&arg));
            }
            return cmd;
        }

        format!(
            "echo [ERROR] Missing start_asi.cmd and bin\\asi.exe in {} && echo. && pause",
            base_dir.display()
        )
    }

    pub fn run() {
        let exe_path = env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
        let base_dir = exe_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        let start_script = base_dir.join("start_asi.cmd");
        let has_args = env::args().len() > 1;
        let command_text = if has_args {
            backend_command(&base_dir)
        } else if start_script.exists() {
            quoted_path(&start_script)
        } else {
            backend_command(&base_dir)
        };

        let _ = Command::new("cmd")
            .arg("/K")
            .arg(command_text)
            .current_dir(&base_dir)
            .spawn();
    }
}

#[cfg(target_os = "windows")]
fn main() {
    win::run();
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("ASI launcher is only supported on Windows.");
}


