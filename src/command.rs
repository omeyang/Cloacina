use anyhow::{Context, Result, bail};
use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

#[derive(Clone, Debug)]
pub struct Invocation {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub remove_env: Vec<&'static str>,
}

impl Invocation {
    pub fn new(program: impl Into<PathBuf>, args: &[&str]) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(OsString::from).collect(),
            remove_env: Vec::new(),
        }
    }

    pub fn display(&self) -> String {
        std::iter::once(self.program.as_os_str())
            .chain(self.args.iter().map(OsString::as_os_str))
            .map(|s| s.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

pub trait Runner {
    fn run(&self, command: &Invocation) -> Result<Output>;
}

pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&self, command: &Invocation) -> Result<Output> {
        let mut process = Command::new(&command.program);
        process.args(&command.args).stdin(Stdio::null());
        for key in &command.remove_env {
            process.env_remove(key);
        }
        process
            .output()
            .with_context(|| format!("执行 {}", command.display()))
    }
}

pub fn checked(runner: &dyn Runner, command: &Invocation) -> Result<String> {
    let output = runner.run(command)?;
    if !output.status.success() {
        bail!(
            "{}: {}{}",
            command.display(),
            String::from_utf8_lossy(&output.stderr).trim(),
            String::from_utf8_lossy(&output.stdout).trim()
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

pub fn which(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|path| executable(path))
}

fn executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}
