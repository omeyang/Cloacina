use anyhow::{Context, Result, bail};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct Activity {
    paths: HashSet<PathBuf>,
}

impl Activity {
    pub fn scan(proc_root: &Path, uid: u32) -> Result<Self> {
        let mut activity = Self::default();
        for entry in fs::read_dir(proc_root).context("读取进程清单")? {
            let entry = entry?;
            if !entry
                .file_name()
                .as_encoded_bytes()
                .iter()
                .all(u8::is_ascii_digit)
            {
                continue;
            }
            let process = entry.path();
            let metadata = match fs::metadata(&process) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            if uid != 0 && metadata.uid() != uid {
                continue;
            }
            // Processes may exit during inspection. Other inspection failures must not
            // turn an unknown activity state into permission to delete data.
            let inspect = (|| -> io::Result<()> {
                for name in ["exe", "cwd"] {
                    match fs::read_link(process.join(name)) {
                        Ok(path) => activity.insert(&path),
                        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                        Err(error) => return Err(error),
                    }
                }
                match fs::read_dir(process.join("fd")) {
                    Ok(descriptors) => {
                        for fd in descriptors {
                            match fs::read_link(fd?.path()) {
                                Ok(path) => activity.insert(&path),
                                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                                Err(error) => return Err(error),
                            }
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                    Err(error) => return Err(error),
                }
                match fs::read_to_string(process.join("maps")) {
                    Ok(maps) => {
                        for line in maps.lines() {
                            let mut rest = line;
                            for _ in 0..5 {
                                rest = rest.trim_start();
                                rest = rest.find(char::is_whitespace).map_or("", |i| &rest[i..]);
                            }
                            activity.insert(Path::new(rest.trim_start()));
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                    Err(error) => return Err(error),
                }
                Ok(())
            })();
            if let Err(error) = inspect {
                if process.exists() {
                    return Err(error).with_context(|| {
                        format!("检查进程 {}", entry.file_name().to_string_lossy())
                    });
                }
            }
        }
        Ok(activity)
    }

    fn insert(&mut self, path: &Path) {
        if !path.is_absolute() {
            return;
        }
        let path = path.to_string_lossy();
        self.paths.insert(PathBuf::from(
            path.strip_suffix(" (deleted)").unwrap_or(&path),
        ));
    }

    pub fn uses(&self, path: &Path) -> bool {
        self.paths.iter().any(|active| active.starts_with(path))
    }
}

/// Hold Cargo's mutate and download locks, in Cargo's acquisition order.
pub struct CargoLocks {
    _files: Vec<File>,
}

impl CargoLocks {
    pub fn acquire(home: &Path) -> Result<Self> {
        Self::lock(home, true)
    }

    pub fn probe(home: &Path) -> Result<Self> {
        Self::lock(home, false)
    }

    fn lock(home: &Path, create: bool) -> Result<Self> {
        let mut files = Vec::new();
        for name in [".package-cache-mutate", ".package-cache"] {
            let file = match OpenOptions::new()
                .read(true)
                .write(create)
                .create(create)
                .truncate(false)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(home.join(name))
            {
                Ok(file) => file,
                Err(error) if !create && error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            // SAFETY: the descriptor is valid and flock does not access Rust memory.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                let error = io::Error::last_os_error();
                bail!("Cargo 缓存正在使用或无法锁定，保留缓存：{error}");
            }
            files.push(file);
        }
        Ok(Self { _files: files })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn process_references_include_cwd_fds_and_mappings_with_spaces() {
        let temp = tempfile::tempdir().unwrap();
        let process = temp.path().join("123");
        fs::create_dir_all(process.join("fd")).unwrap();
        symlink("/versions/old/bin/tool", process.join("exe")).unwrap();
        symlink("/dev/shm/work", process.join("cwd")).unwrap();
        symlink("/dev/shm/fd/file (deleted)", process.join("fd/3")).unwrap();
        fs::write(
            process.join("maps"),
            "123-456 r--p 0 00:00 42   /dev/shm/mapped file\n",
        )
        .unwrap();
        let activity = Activity::scan(temp.path(), 0).unwrap();
        for path in [
            "/versions/old",
            "/dev/shm/work",
            "/dev/shm/fd",
            "/dev/shm/mapped file",
        ] {
            assert!(activity.uses(Path::new(path)), "{path}");
        }
        assert!(!activity.uses(Path::new("/dev/shm/unused")));
    }

    #[test]
    fn cargo_locks_exclude_other_cleaners_and_are_released_on_drop() {
        let temp = tempfile::tempdir().unwrap();
        let locks = CargoLocks::acquire(temp.path()).unwrap();
        assert!(CargoLocks::acquire(temp.path()).is_err());
        drop(locks);
        assert!(CargoLocks::acquire(temp.path()).is_ok());
        assert!(temp.path().join(".package-cache").exists());
    }
}
