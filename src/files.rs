use crate::activity::{Activity, CargoLocks};
use crate::cli::Options;
use anyhow::{Context, Result, bail};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::{self, Metadata};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

const HOME_FILES: &[&str] = &[
    ".tig_history",
    ".lesshst",
    ".wget-hsts",
    ".viminfo",
    ".my-git-sync-cron.sh",
    ".shell.pre-oh-my-zsh",
    ".roo",
    ".costrict",
    ".dotnet",
    "gobuild-verify.log",
    ".venvs",
    ".cshrc",
    ".tcshrc",
    ".ksshrc",
    ".bash_history",
    ".bashrc",
    ".bash_profile",
    ".bash_logout",
];

#[derive(Clone, Debug)]
pub struct Roots {
    pub home: PathBuf,
    pub tmp: PathBuf,
    pub logs: PathBuf,
    pub shm: PathBuf,
    pub proc: PathBuf,
    pub cargo_home: PathBuf,
    pub uid: u32,
    pub mounts: Vec<PathBuf>,
}

impl Roots {
    pub fn from_environment() -> Result<Self> {
        let home = std::env::var_os("HOME").context("HOME 未设置")?;
        let home = fs::canonicalize(home).context("家目录不可访问")?;
        if home == Path::new("/") || !home.is_dir() {
            bail!("拒绝将 / 或非目录作为家目录");
        }
        // SAFETY: geteuid has no arguments or memory-safety preconditions.
        let uid = unsafe { libc::geteuid() };
        Ok(Self {
            cargo_home: std::env::var_os("CARGO_HOME")
                .map(PathBuf::from)
                .map(|path| -> std::io::Result<PathBuf> {
                    if path.is_absolute() {
                        Ok(path)
                    } else {
                        Ok(std::env::current_dir()?.join(path))
                    }
                })
                .transpose()?
                .unwrap_or_else(|| home.join(".cargo")),
            home,
            shm: "/dev/shm".into(),
            proc: "/proc".into(),
            tmp: "/tmp".into(),
            logs: "/var/log".into(),
            uid,
            mounts: mount_points()?,
        })
    }
}

fn mount_points() -> Result<Vec<PathBuf>> {
    use std::os::unix::ffi::OsStringExt;
    let data = fs::read("/proc/self/mountinfo").context("读取挂载点，避免清理挂载目录")?;
    Ok(data
        .split(|b| *b == b'\n')
        .filter_map(|line| {
            let field = line.split(|b| *b == b' ').nth(4)?;
            let mut bytes = Vec::new();
            let mut i = 0;
            while i < field.len() {
                if field[i] == b'\\'
                    && i + 3 < field.len()
                    && field[i + 1..i + 4]
                        .iter()
                        .all(|b| (b'0'..=b'7').contains(b))
                {
                    let code =
                        (field[i + 1] - b'0') * 64 + (field[i + 2] - b'0') * 8 + field[i + 3]
                            - b'0';
                    bytes.push(code);
                    i += 4;
                } else {
                    bytes.push(field[i]);
                    i += 1;
                }
            }
            Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
        })
        .collect())
}

#[derive(Clone, Debug, PartialEq)]
struct Identity {
    dev: u64,
    ino: u64,
    modified: i64,
    nanos: i64,
    len: u64,
    mode: u32,
    uid: u32,
}

impl From<&Metadata> for Identity {
    fn from(meta: &Metadata) -> Self {
        Self {
            dev: meta.dev(),
            ino: meta.ino(),
            modified: meta.mtime(),
            nanos: meta.mtime_nsec(),
            len: meta.len(),
            mode: meta.mode(),
            uid: meta.uid(),
        }
    }
}

#[derive(Clone, Debug)]
enum Policy {
    Regular,
    Temporary {
        days: u64,
        root: PathBuf,
    },
    Version {
        current: PathBuf,
        versions: PathBuf,
        root: PathBuf,
    },
    CargoCache,
}

#[derive(Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub section: &'static str,
    pub bytes: u64,
    identity: Identity,
    policy: Policy,
}

#[derive(Default, Debug)]
pub struct Plan {
    pub entries: Vec<Entry>,
    pub notices: Vec<String>,
}

struct Planner<'a> {
    roots: &'a Roots,
    now: SystemTime,
    keep: Vec<PathBuf>,
    seen: HashSet<PathBuf>,
    plan: Plan,
    activity: Option<Activity>,
}

impl Plan {
    pub fn build(roots: &Roots, options: &Options) -> Result<Self> {
        let mut keep = vec![
            roots.home.join(".cache/gitstatus"),
            roots.home.join(".zsh_history"),
        ];
        for path in &options.keep {
            let path = if let Ok(rest) = path.strip_prefix("~") {
                roots.home.join(rest)
            } else if path.is_absolute() {
                path.clone()
            } else {
                roots.home.join(path)
            };
            keep.push(normalize(&path)?);
        }
        let mut planner = Planner {
            roots,
            now: SystemTime::now(),
            keep,
            seen: HashSet::new(),
            plan: Self::default(),
            activity: None,
        };
        if !options.targeted() {
            planner.basic(options);
        }
        if options.deep || options.all || options.targeted() {
            match Activity::scan(&roots.proc, roots.uid) {
                Ok(activity) => planner.activity = Some(activity),
                Err(error) => planner.plan.notices.push(format!(
                    "无法核实进程占用，保留共享内存、工具旧版本和 Cargo 缓存：{error:#}"
                )),
            }
            if planner.activity.is_some() {
                if options.deep || options.all || options.old_versions {
                    planner.old_binaries();
                }
                if options.deep || options.all || options.cargo_cache {
                    planner.cargo_cache();
                }
                if options.deep || options.all || options.memory {
                    planner.shared_memory(options.shm_days);
                }
            }
        }
        if (options.system || options.all) && roots.uid == 0 {
            planner.old_files(&roots.logs, 1, FileFilter::Logs, "轮转系统日志", false);
        }
        Ok(planner.plan)
    }

    pub fn execute(&self, roots: &Roots, options: &Options) -> usize {
        for notice in &self.notices {
            println!("  跳过：{notice}");
        }
        let mut section = "";
        let mut done = 0;
        let mut errors = 0;
        let mut bytes = 0;
        let mut memory_bytes = 0;
        // Retain these locks through the whole deletion pass. A build must not
        // start reading caches in between two cache-directory removals.
        let mut cargo_locks = None;
        let mut cargo_lock_failed = false;
        for entry in &self.entries {
            if section != entry.section {
                section = entry.section;
                println!("\n▸ {section}");
            }
            if options.dry_run {
                println!(
                    "  将删除  {}  ({})",
                    entry.path.display(),
                    human(entry.bytes)
                );
                done += 1;
                if entry.path.starts_with(&roots.shm) {
                    memory_bytes += entry.bytes;
                } else {
                    bytes += entry.bytes;
                }
                continue;
            }
            if matches!(entry.policy, Policy::CargoCache) && cargo_locks.is_none() {
                if cargo_lock_failed {
                    continue;
                }
                match CargoLocks::acquire(&roots.cargo_home) {
                    Ok(locks) => cargo_locks = Some(locks),
                    Err(error) => {
                        eprintln!("  未删除 Cargo 缓存：{error:#}");
                        errors += 1;
                        cargo_lock_failed = true;
                        continue;
                    }
                }
            }
            match delete_entry(entry, roots) {
                Ok(()) => {
                    println!(
                        "  已删除  {}  ({})",
                        entry.path.display(),
                        human(entry.bytes)
                    );
                    done += 1;
                    if entry.path.starts_with(&roots.shm) {
                        memory_bytes += entry.bytes;
                    } else {
                        bytes += entry.bytes;
                    }
                }
                Err(error) => {
                    eprintln!("  未删除 {}：{error:#}", entry.path.display());
                    errors += 1;
                }
            }
        }
        println!(
            "\n文件{}：{done} 项，估算磁盘 {}，共享内存 {}。",
            if options.dry_run { "预演" } else { "清理" },
            human(bytes),
            human(memory_bytes)
        );
        errors
    }
}

fn normalize(path: &Path) -> Result<PathBuf> {
    let mut normalized = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => (),
            Component::ParentDir => {
                if !normalized.pop() {
                    bail!("保留路径越过根目录");
                }
            }
            _ => normalized.push(part),
        }
    }
    Ok(normalized)
}

fn no_symlink_parents(path: &Path) -> Result<()> {
    for parent in path.ancestors().skip(1) {
        if fs::symlink_metadata(parent)?.file_type().is_symlink() {
            bail!("父目录是符号链接：{}", parent.display());
        }
    }
    Ok(())
}

fn contains_mount(path: &Path, mounts: &[PathBuf]) -> bool {
    mounts.iter().any(|mount| mount.starts_with(path))
}

fn children(path: &Path) -> Result<Vec<PathBuf>> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => (),
        Ok(_) => return Ok(Vec::new()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    }
    no_symlink_parents(path)?;
    let mut paths = fs::read_dir(path)?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.sort();
    Ok(paths)
}

fn allocated(path: &Path, dev: u64) -> Result<u64> {
    let meta = fs::symlink_metadata(path)?;
    if meta.dev() != dev {
        bail!("目录跨越文件系统：{}", path.display());
    }
    let mut bytes = meta.blocks().saturating_mul(512);
    if meta.is_dir() {
        for child in children(path)? {
            bytes = bytes.saturating_add(allocated(&child, dev)?);
        }
    }
    Ok(bytes)
}

fn old(meta: &Metadata, days: u64, now: SystemTime) -> bool {
    meta.modified()
        .ok()
        .and_then(|mtime| now.duration_since(mtime).ok())
        .is_some_and(|age| age > Duration::from_secs(days.saturating_mul(86_400)))
}

fn protected_tmp_name(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    name.starts_with("systemd-private-")
        || name.starts_with("sem.")
        || name.starts_with("aliyun-assist-")
        || name.starts_with("ssh-")
        || name.ends_with(".sock")
        || name.ends_with(".lock")
        || name.ends_with("-lock")
        || matches!(
            name.as_ref(),
            "claude-0"
                | "libpod_lock"
                | ".X11-unix"
                | ".ICE-unix"
                | ".font-unix"
                | ".XIM-unix"
                | ".Test-unix"
        )
}

fn temporary_tree_is_old(
    path: &Path,
    uid: u32,
    days: u64,
    now: SystemTime,
    dev: u64,
) -> Result<bool> {
    let meta = fs::symlink_metadata(path)?;
    let kind = meta.file_type();
    if meta.uid() != uid
        || meta.dev() != dev
        || !old(&meta, days, now)
        || kind.is_socket()
        || kind.is_fifo()
        || kind.is_block_device()
        || kind.is_char_device()
        || path.file_name().is_some_and(protected_tmp_name)
    {
        return Ok(false);
    }
    if kind.is_dir() {
        for child in children(path)? {
            if !temporary_tree_is_old(&child, uid, days, now, dev)? {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

#[derive(Clone, Copy)]
enum FileFilter {
    All,
    Jsonl,
    Logs,
}

impl FileFilter {
    fn matches(self, path: &Path) -> bool {
        match self {
            Self::All => true,
            Self::Jsonl => path.extension() == Some(OsStr::new("jsonl")),
            Self::Logs => path.extension().is_some_and(|ext| {
                let ext = ext.to_string_lossy();
                matches!(ext.as_ref(), "gz" | "xz" | "zst")
                    || (!ext.is_empty() && ext.bytes().all(|b| b.is_ascii_digit()))
            }),
        }
    }
}

impl Planner<'_> {
    fn children(&mut self, path: &Path) -> Vec<PathBuf> {
        match children(path) {
            Ok(paths) => paths,
            Err(error) => {
                self.plan
                    .notices
                    .push(format!("{}：{error:#}", path.display()));
                Vec::new()
            }
        }
    }

    fn add(&mut self, path: PathBuf, section: &'static str, policy: Policy) {
        if self.keep.iter().any(|keep| path.starts_with(keep)) || self.seen.contains(&path) {
            return;
        }
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => {
                self.plan
                    .notices
                    .push(format!("{}：{error}", path.display()));
                return;
            }
        };
        if contains_mount(&path, &self.roots.mounts) {
            self.plan
                .notices
                .push(format!("{} 是挂载点或包含挂载点", path.display()));
            return;
        }
        if self.keep.iter().any(|keep| keep.starts_with(&path)) {
            if meta.is_dir() {
                for child in self.children(&path) {
                    self.add(child, section, policy.clone());
                }
            }
            return;
        }
        let bytes = no_symlink_parents(&path).and_then(|()| allocated(&path, meta.dev()));
        match bytes {
            Ok(bytes) => {
                self.seen.insert(path.clone());
                self.plan.entries.push(Entry {
                    path,
                    section,
                    bytes,
                    identity: Identity::from(&meta),
                    policy,
                });
            }
            Err(error) => self
                .plan
                .notices
                .push(format!("{}：{error:#}", path.display())),
        }
    }

    fn basic(&mut self, options: &Options) {
        for name in HOME_FILES {
            self.add(
                self.roots.home.join(name),
                "家目录指定文件与虚拟环境",
                Policy::Regular,
            );
        }
        for path in self.children(&self.roots.home) {
            let name = path.file_name().unwrap().to_string_lossy();
            if name.starts_with(".zcompdump") || name.starts_with(".claude.json.tmp.") {
                self.add(path, "家目录指定文件与虚拟环境", Policy::Regular);
            }
        }
        let cache = self.roots.home.join(".cache");
        if fs::symlink_metadata(&cache).is_ok_and(|m| m.file_type().is_symlink()) {
            self.plan
                .notices
                .push(format!("{} 是符号链接，保留目标目录", cache.display()));
        }
        for path in self.children(&cache) {
            self.add(path, "家目录缓存（保留 gitstatus）", Policy::Regular);
        }
        self.old_files(
            &self.roots.home.join(".codex/sessions"),
            options.keep_days,
            FileFilter::All,
            "过期 AI 会话历史",
            false,
        );
        self.old_files(
            &self.roots.home.join(".claude/projects"),
            options.keep_days,
            FileFilter::Jsonl,
            "过期 AI 会话历史",
            true,
        );
        for name in [
            ".codex/shell_snapshots",
            ".claude/shell-snapshots",
            ".claude/paste-cache",
        ] {
            self.add(
                self.roots.home.join(name),
                "可重建状态与快照",
                Policy::Regular,
            );
        }
        for path in self.children(&self.roots.home.join(".codex")) {
            if path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("logs_2.sqlite")
            {
                self.add(path, "可重建状态与快照", Policy::Regular);
            }
        }
        for path in self.children(&self.roots.tmp) {
            self.old_tmp(path, options.tmp_days);
        }
        for project in self.children(&self.roots.tmp.join("claude-0")) {
            for session in self.children(&project) {
                self.old_tmp(session, options.tmp_days);
            }
        }
    }

    fn old_tmp(&mut self, path: PathBuf, days: u64) {
        if contains_mount(&path, &self.roots.mounts) {
            return;
        }
        let Ok(meta) = fs::symlink_metadata(&path) else {
            return;
        };
        if temporary_tree_is_old(&path, self.roots.uid, days, self.now, meta.dev()).unwrap_or(false)
        {
            self.add(
                path.clone(),
                "过期临时文件（保留活跃端点及近期内容）",
                Policy::Temporary { days, root: path },
            );
        }
    }

    fn old_files(
        &mut self,
        root: &Path,
        days: u64,
        filter: FileFilter,
        section: &'static str,
        protect_memory: bool,
    ) {
        for path in self.children(root) {
            if protect_memory && path.file_name() == Some(OsStr::new("memory")) {
                continue;
            }
            if contains_mount(&path, &self.roots.mounts) {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                self.old_files(&path, days, filter, section, protect_memory);
            } else if meta.is_file() && filter.matches(&path) && old(&meta, days, self.now) {
                self.add(path, section, Policy::Regular);
            }
        }
    }

    fn shared_memory(&mut self, days: u64) {
        for path in self.children(&self.roots.shm) {
            self.shared_temporary(path.clone(), days, &path);
        }
    }

    fn shared_temporary(&mut self, path: PathBuf, days: u64, root: &Path) {
        if contains_mount(&path, &self.roots.mounts) {
            return;
        }
        if self
            .activity
            .as_ref()
            .is_some_and(|activity| activity.uses(&path))
        {
            self.plan
                .notices
                .push(format!("{} 仍被进程使用", path.display()));
            return;
        }
        let Ok(meta) = fs::symlink_metadata(&path) else {
            return;
        };
        if meta.uid() != self.roots.uid
            || !old(&meta, days, self.now)
            || path.file_name().is_some_and(protected_tmp_name)
        {
            return;
        }
        if temporary_tree_is_old(&path, self.roots.uid, days, self.now, meta.dev()).unwrap_or(false)
        {
            self.add(
                path,
                "共享内存中的过期临时文件",
                Policy::Temporary {
                    days,
                    root: root.to_owned(),
                },
            );
        } else if meta.is_dir() {
            // A retained lock file must not keep gigabytes of unrelated, idle
            // build output alive. Recurse without removing the protected entry.
            for child in self.children(&path) {
                self.shared_temporary(child, days, root);
            }
        }
    }

    fn cargo_cache(&mut self) {
        if !self.roots.cargo_home.is_absolute() || self.roots.cargo_home == Path::new("/") {
            self.plan.notices.push("Cargo 家目录无效，保留缓存".into());
            return;
        }
        if let Err(error) = CargoLocks::probe(&self.roots.cargo_home) {
            self.plan
                .notices
                .push(format!("保留 Cargo 缓存：{error:#}"));
            return;
        }
        for name in ["registry", "git", "advisory-db"] {
            let path = self.roots.cargo_home.join(name);
            if self
                .activity
                .as_ref()
                .is_some_and(|activity| activity.uses(&path))
            {
                self.plan
                    .notices
                    .push(format!("{} 仍被进程使用", path.display()));
                continue;
            }
            if fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_symlink()) {
                self.plan
                    .notices
                    .push(format!("{} 是符号链接，保留缓存", path.display()));
                continue;
            }
            self.add(
                path,
                "Cargo 依赖与检查缓存（保留 bin、配置、凭据和锁文件）",
                Policy::CargoCache,
            );
        }
    }

    fn versioned(&mut self, versions: &Path, current_link: &Path) {
        if !versions.exists() {
            return;
        }
        let resolve = || -> Result<(PathBuf, PathBuf)> {
            let current = fs::canonicalize(current_link)?;
            let directory = fs::canonicalize(versions)?;
            if current == directory || !current.starts_with(&directory) {
                bail!("当前版本不在版本目录内");
            }
            Ok((current, directory))
        };
        let (current, directory) = match resolve() {
            Ok(paths) => paths,
            Err(error) => {
                self.plan.notices.push(format!(
                    "无法识别 {} 当前版本，保留所有版本：{error:#}",
                    versions.display()
                ));
                return;
            }
        };
        for version in self.children(versions) {
            if fs::symlink_metadata(&version).is_ok_and(|meta| meta.file_type().is_symlink()) {
                continue;
            }
            if current.starts_with(&version) {
                continue;
            }
            if self
                .activity
                .as_ref()
                .is_some_and(|activity| activity.uses(&version))
            {
                self.plan
                    .notices
                    .push(format!("{} 仍被进程使用，保留该版本", version.display()));
                continue;
            }
            self.add(
                version.clone(),
                "旧版本二进制",
                Policy::Version {
                    current: current_link.to_owned(),
                    versions: directory.clone(),
                    root: version,
                },
            );
        }
    }

    fn old_binaries(&mut self) {
        for package in ["standalone", "app-server-daemon"] {
            let root = self.roots.home.join(".codex/packages").join(package);
            self.versioned(&root.join("releases"), &root.join("current"));
        }
        self.versioned(
            &self.roots.home.join(".local/share/claude/versions"),
            &self.roots.home.join(".local/bin/claude"),
        );
        let server = self.roots.home.join(".vscode-server");
        let lru = server.join("cli/servers/lru.json");
        if !lru.exists() {
            return;
        }
        let parse = fs::read(&lru)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| Ok(serde_json::from_slice::<Vec<String>>(&bytes)?));
        match parse {
            Ok(versions) => {
                let Some(current) = versions
                    .first()
                    .filter(|s| s.starts_with("Stable-") && s.len() > 7)
                else {
                    self.plan
                        .notices
                        .push("无法识别 VS Code 当前版本，保留所有版本".into());
                    return;
                };
                for path in self.children(&server.join("cli/servers")) {
                    let name = path.file_name().unwrap().to_string_lossy();
                    if name.starts_with("Stable-") && name != *current {
                        self.add(path, "旧版本二进制", Policy::Regular);
                    }
                }
                let current_cli = format!("code-{}", current.trim_start_matches("Stable-"));
                for path in self.children(&server) {
                    let name = path.file_name().unwrap().to_string_lossy();
                    if name.starts_with("code-") && name != current_cli {
                        self.add(path, "旧版本二进制", Policy::Regular);
                    }
                }
            }
            Err(error) => self
                .plan
                .notices
                .push(format!("无法读取 VS Code 版本清单，保留所有版本：{error}")),
        }
    }
}

fn delete_entry(entry: &Entry, roots: &Roots) -> Result<()> {
    no_symlink_parents(&entry.path)?;
    if contains_mount(&entry.path, &mount_points()?) {
        bail!("路径包含挂载点");
    }
    let meta = fs::symlink_metadata(&entry.path)?;
    if Identity::from(&meta) != entry.identity {
        bail!("路径在扫描后发生变化，请重新扫描");
    }
    if !matches!(entry.policy, Policy::Regular) {
        let activity = Activity::scan(&roots.proc, roots.uid)?;
        let activity_root = match &entry.policy {
            Policy::Temporary { root, .. } | Policy::Version { root, .. } => root,
            _ => &entry.path,
        };
        if activity.uses(activity_root) {
            bail!("路径仍被进程使用");
        }
    }
    match &entry.policy {
        Policy::Temporary { days, .. } => {
            if !temporary_tree_is_old(&entry.path, roots.uid, *days, SystemTime::now(), meta.dev())?
            {
                bail!("临时目录已有近期内容或活跃端点");
            }
        }
        Policy::Version {
            current,
            versions,
            root,
        } => {
            let current = fs::canonicalize(current).context("无法重新确认当前工具版本")?;
            let versions = fs::canonicalize(versions)?;
            if current == versions || !current.starts_with(&versions) {
                bail!("当前工具版本不在版本目录内，保留所有版本");
            }
            if current.starts_with(fs::canonicalize(root)?) {
                bail!("该版本已成为当前版本，保留");
            }
        }
        _ => (),
    }
    if meta.is_dir() {
        fs::remove_dir_all(&entry.path)?;
    } else {
        fs::remove_file(&entry.path)?;
    }
    Ok(())
}

pub fn human(bytes: u64) -> String {
    if bytes >= 1 << 30 {
        format!("{:.1} GiB", bytes as f64 / (1_u64 << 30) as f64)
    } else if bytes >= 1 << 20 {
        format!("{:.1} MiB", bytes as f64 / (1_u64 << 20) as f64)
    } else if bytes >= 1 << 10 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::fs::{File, FileTimes};
    use std::os::unix::fs::symlink;
    use std::os::unix::net::UnixListener;
    use tempfile::TempDir;

    fn fixture() -> (TempDir, Roots) {
        let temp = tempfile::tempdir().unwrap();
        let roots = Roots {
            home: temp.path().join("home with spaces"),
            tmp: temp.path().join("tmp"),
            logs: temp.path().join("logs"),
            shm: temp.path().join("shm"),
            proc: temp.path().join("proc"),
            cargo_home: temp.path().join("home with spaces/.cargo"),
            uid: fs::metadata(temp.path()).unwrap().uid(),
            mounts: vec![],
        };
        for path in [
            &roots.home,
            &roots.tmp,
            &roots.logs,
            &roots.shm,
            &roots.proc,
        ] {
            fs::create_dir_all(path).unwrap();
        }
        (temp, roots)
    }
    fn file(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "data").unwrap();
    }
    fn age(path: &Path) {
        File::open(path)
            .unwrap()
            .set_times(
                FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(40 * 86400)),
            )
            .unwrap();
    }
    fn opts(args: &[&str]) -> Options {
        Options::parse_from(std::iter::once("cloacina").chain(args.iter().copied()))
    }

    #[test]
    fn requested_files_removed_gitstatus_and_memory_preserved() {
        let (_temp, roots) = fixture();
        let removed = [
            ".bashrc",
            ".bash_profile",
            ".bash_logout",
            ".bash_history",
            ".cshrc",
            ".tcshrc",
            ".shell.pre-oh-my-zsh",
            ".zcompdump-localhost-5.9",
            ".zcompdump-localhost-5.9.zwc",
            ".venvs/test/config",
            ".cache/p10k-dump",
            ".cache/..hidden",
            ".cache/data with spaces/cache",
            ".claude/projects/demo/old.jsonl",
        ];
        let kept = [
            ".cache/gitstatus/gitstatusd",
            ".zsh_history",
            ".zshrc",
            ".claude/projects/demo/memory/old.jsonl",
        ];
        for name in removed.iter().chain(kept.iter()) {
            let path = roots.home.join(name);
            file(&path);
            age(&path);
        }
        let options = opts(&[]);
        let plan = Plan::build(&roots, &options).unwrap();
        assert_eq!(plan.execute(&roots, &opts(&["-n"])), 0);
        assert!(removed.iter().all(|p| roots.home.join(p).exists()));
        assert_eq!(plan.execute(&roots, &options), 0);
        assert!(removed.iter().all(|p| !roots.home.join(p).exists()));
        assert!(kept.iter().all(|p| roots.home.join(p).exists()));
    }

    #[test]
    fn keep_descendant_removes_only_its_siblings() {
        let (_temp, roots) = fixture();
        file(&roots.home.join(".venvs/keep/config"));
        file(&roots.home.join(".venvs/remove/config"));
        let options = opts(&["--keep", ".venvs/keep"]);
        assert_eq!(
            Plan::build(&roots, &options)
                .unwrap()
                .execute(&roots, &options),
            0
        );
        assert!(roots.home.join(".venvs/keep/config").exists());
        assert!(!roots.home.join(".venvs/remove").exists());
    }

    #[test]
    fn symlink_targets_and_replaced_paths_are_not_deleted() {
        let (temp, roots) = fixture();
        let external = temp.path().join("external");
        file(&external.join("data"));
        symlink(&external, roots.home.join(".cache")).unwrap();
        symlink(&external, roots.home.join(".venvs")).unwrap();
        file(&roots.home.join(".bashrc"));
        let options = opts(&[]);
        let plan = Plan::build(&roots, &options).unwrap();
        fs::remove_file(roots.home.join(".bashrc")).unwrap();
        symlink(&external, roots.home.join(".bashrc")).unwrap();
        assert_eq!(plan.execute(&roots, &options), 1);
        assert!(external.join("data").exists());
        assert!(roots.home.join(".cache").is_symlink());
        assert!(roots.home.join(".bashrc").is_symlink());
        assert!(!roots.home.join(".venvs").exists());
    }

    #[test]
    fn tmp_keeps_recent_children_endpoints_locks_and_mounts() {
        let (_temp, mut roots) = fixture();
        for name in [
            "old/file",
            "busy/recent",
            "locked/data.lock",
            "mounted/data",
        ] {
            file(&roots.tmp.join(name));
            if name != "busy/recent" {
                age(&roots.tmp.join(name));
            }
        }
        fs::create_dir_all(roots.tmp.join("socket-dir")).unwrap();
        let _socket = UnixListener::bind(roots.tmp.join("socket-dir/socket")).unwrap();
        for path in children(&roots.tmp).unwrap() {
            age(&path);
        }
        roots.mounts.push(roots.tmp.join("mounted/data"));
        let options = opts(&[]);
        assert_eq!(
            Plan::build(&roots, &options)
                .unwrap()
                .execute(&roots, &options),
            0
        );
        assert!(!roots.tmp.join("old").exists());
        for path in [
            "busy/recent",
            "locked/data.lock",
            "socket-dir/socket",
            "mounted/data",
        ] {
            assert!(roots.tmp.join(path).exists());
        }
        assert!(roots.tmp.is_dir());
    }

    #[test]
    fn tmp_is_rechecked_before_removal() {
        let (_temp, roots) = fixture();
        let dir = roots.tmp.join("old");
        file(&dir.join("data"));
        age(&dir.join("data"));
        age(&dir);
        let options = opts(&[]);
        let plan = Plan::build(&roots, &options).unwrap();
        file(&dir.join("new-file"));
        assert_eq!(plan.execute(&roots, &options), 1);
        assert!(dir.join("new-file").exists());
    }

    #[test]
    fn deep_keeps_current_binaries_and_parses_formatted_json() {
        let (_temp, roots) = fixture();
        let standalone = roots.home.join(".codex/packages/standalone");
        file(&standalone.join("releases/current-version/bin"));
        file(&standalone.join("releases/old-version/bin"));
        symlink("releases/current-version", standalone.join("current")).unwrap();
        let server = roots.home.join(".vscode-server");
        file(&server.join("cli/servers/Stable-new/server"));
        file(&server.join("cli/servers/Stable-old/server"));
        fs::write(
            server.join("cli/servers/lru.json"),
            "[\n  \"Stable-new\",\n  \"Stable-old\"\n]",
        )
        .unwrap();
        file(&server.join("code-new"));
        file(&server.join("code-old"));
        let options = opts(&["--deep"]);
        assert_eq!(
            Plan::build(&roots, &options)
                .unwrap()
                .execute(&roots, &options),
            0
        );
        assert!(standalone.join("releases/current-version/bin").exists());
        assert!(!standalone.join("releases/old-version").exists());
        assert!(server.join("cli/servers/Stable-new/server").exists());
        assert!(server.join("code-new").exists());
        assert!(!server.join("cli/servers/Stable-old").exists());
        assert!(!server.join("code-old").exists());
    }

    #[test]
    fn old_logs_only_and_non_root_skips_system() {
        let (_temp, mut roots) = fixture();
        for name in ["active.log", "old.log.1", "old.log.gz", "recent.log.gz"] {
            file(&roots.logs.join(name));
            if name != "recent.log.gz" {
                age(&roots.logs.join(name));
            }
        }
        let options = opts(&["--system"]);
        roots.uid = 12345;
        assert!(Plan::build(&roots, &options).unwrap().entries.is_empty());
        roots.uid = 0;
        let plan = Plan::build(&roots, &options).unwrap();
        assert_eq!(plan.entries.len(), 2);
        assert!(
            plan.entries
                .iter()
                .all(|e| e.path != roots.logs.join("active.log"))
        );
    }

    fn process(roots: &Roots) -> PathBuf {
        let process = roots.proc.join("123");
        fs::create_dir_all(process.join("fd")).unwrap();
        fs::write(process.join("maps"), "").unwrap();
        process
    }

    #[test]
    fn shared_memory_keeps_recent_active_protected_and_mounted_data() {
        let (_temp, mut roots) = fixture();
        let process = process(&roots);
        for name in [
            "unused/file",
            "open/file",
            "cwd/file",
            "mapped/file",
            "recent/file",
            "locked/data.lock",
            "mounted/file",
            "sem.app",
            "libpod_lock",
            "keep/file",
        ] {
            file(&roots.shm.join(name));
            if name != "recent/file" {
                age(&roots.shm.join(name));
            }
        }
        fs::create_dir_all(roots.shm.join("endpoint")).unwrap();
        let _socket = UnixListener::bind(roots.shm.join("endpoint/socket")).unwrap();
        for path in children(&roots.shm).unwrap() {
            age(&path);
        }
        symlink(roots.shm.join("open/file"), process.join("fd/3")).unwrap();
        symlink(roots.shm.join("cwd"), process.join("cwd")).unwrap();
        fs::write(
            process.join("maps"),
            format!(
                "123-456 r--p 0 00:00 42 {}\n",
                roots.shm.join("mapped/file").display()
            ),
        )
        .unwrap();
        roots.mounts.push(roots.shm.join("mounted"));
        file(&roots.home.join(".bashrc"));
        let options = opts(&[
            "--memory",
            "--keep",
            roots.shm.join("keep").to_str().unwrap(),
        ]);
        let plan = Plan::build(&roots, &options).unwrap();
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.entries[0].path, roots.shm.join("unused"));
        assert_eq!(plan.execute(&roots, &options), 0);
        assert!(!roots.shm.join("unused").exists());
        assert!(roots.home.join(".bashrc").exists());
        assert!(roots.shm.join("open/file").exists());
        assert!(roots.shm.join("recent/file").exists());
        assert!(roots.shm.join("sem.app").exists());
        assert!(roots.shm.join("libpod_lock").exists());
    }

    #[test]
    fn shared_memory_rechecks_process_activity_after_preview() {
        let (_temp, roots) = fixture();
        file(&roots.shm.join("work/file"));
        age(&roots.shm.join("work/file"));
        age(&roots.shm.join("work"));
        let options = opts(&["--memory"]);
        let plan = Plan::build(&roots, &options).unwrap();
        assert_eq!(plan.entries.len(), 1);
        let process = process(&roots);
        symlink(roots.shm.join("work/file"), process.join("fd/3")).unwrap();
        assert_eq!(plan.execute(&roots, &options), 1);
        assert!(roots.shm.join("work/file").exists());
    }

    #[test]
    fn shared_memory_cleans_around_locks_and_rechecks_activity_in_the_parent() {
        let (_temp, roots) = fixture();
        for name in [
            "build/output/data",
            "build/session.lock",
            "build/.cargo-lock",
        ] {
            file(&roots.shm.join(name));
            age(&roots.shm.join(name));
        }
        age(&roots.shm.join("build/output"));
        age(&roots.shm.join("build"));
        let options = opts(&["--memory"]);
        let plan = Plan::build(&roots, &options).unwrap();
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.execute(&roots, &options), 0);
        assert!(!roots.shm.join("build/output").exists());
        assert!(roots.shm.join("build/session.lock").exists());
        assert!(roots.shm.join("build/.cargo-lock").exists());
        file(&roots.shm.join("build/output/data"));
        for name in ["build/output/data", "build/output", "build"] {
            age(&roots.shm.join(name));
        }
        let plan = Plan::build(&roots, &options).unwrap();
        let process = process(&roots);
        symlink(roots.shm.join("build"), process.join("cwd")).unwrap();
        assert_eq!(plan.execute(&roots, &options), 1);
        assert!(roots.shm.join("build/output/data").exists());
    }

    #[test]
    fn current_and_running_codex_and_claude_versions_are_preserved() {
        let (_temp, roots) = fixture();
        let process = process(&roots);
        for package in ["standalone", "app-server-daemon"] {
            let root = roots.home.join(".codex/packages").join(package);
            for name in ["old", "running", "current-version"] {
                file(&root.join("releases").join(name).join("bin/tool"));
            }
            symlink("releases/current-version", root.join("current")).unwrap();
            let fd = if package == "standalone" { "3" } else { "4" };
            symlink(
                root.join("releases/running/bin/tool"),
                process.join("fd").join(fd),
            )
            .unwrap();
        }
        let versions = roots.home.join(".local/share/claude/versions");
        for version in ["1.0", "2.0"] {
            file(&versions.join(version));
        }
        fs::create_dir_all(roots.home.join(".local/bin")).unwrap();
        symlink(versions.join("2.0"), roots.home.join(".local/bin/claude")).unwrap();
        let options = opts(&["--old-versions"]);
        let plan = Plan::build(&roots, &options).unwrap();
        assert_eq!(plan.entries.len(), 3);
        assert_eq!(plan.execute(&roots, &options), 0);
        for package in ["standalone", "app-server-daemon"] {
            let root = roots.home.join(".codex/packages").join(package);
            assert!(!root.join("releases/old").exists());
            assert!(root.join("releases/running/bin/tool").exists());
            assert!(root.join("releases/current-version/bin/tool").exists());
        }
        assert!(!versions.join("1.0").exists());
        assert!(versions.join("2.0").exists());
    }

    #[test]
    fn versions_are_preserved_when_current_is_unknown_or_changes_after_scan() {
        let (_temp, roots) = fixture();
        let root = roots.home.join(".codex/packages/app-server-daemon");
        file(&root.join("releases/old/bin/tool"));
        file(&root.join("releases/old/keep"));
        file(&root.join("releases/new/bin/tool"));
        let options = opts(&[
            "--old-versions",
            "--keep",
            root.join("releases/old/keep").to_str().unwrap(),
        ]);
        assert!(Plan::build(&roots, &options).unwrap().entries.is_empty());
        symlink("releases/new", root.join("current")).unwrap();
        let plan = Plan::build(&roots, &options).unwrap();
        fs::remove_file(root.join("current")).unwrap();
        symlink("releases/old", root.join("current")).unwrap();
        assert_eq!(plan.execute(&roots, &options), 1);
        assert!(root.join("releases/old/bin/tool").exists());
    }

    #[test]
    fn cargo_preview_is_read_only_and_cleanup_preserves_tools_configuration_and_locks() {
        let (_temp, mut roots) = fixture();
        roots.cargo_home = roots.home.join("custom-cargo");
        for path in [
            "registry/src/package/source",
            "git/checkouts/package/source",
            "advisory-db/data",
            "bin/tool",
            "config.toml",
            "credentials.toml",
            ".crates.toml",
        ] {
            file(&roots.cargo_home.join(path));
        }
        let options = opts(&["--cargo-cache"]);
        let plan = Plan::build(&roots, &options).unwrap();
        assert_eq!(plan.entries.len(), 3);
        assert_eq!(plan.execute(&roots, &opts(&["--cargo-cache", "-n"])), 0);
        assert!(!roots.cargo_home.join(".package-cache").exists());
        assert_eq!(plan.execute(&roots, &options), 0);
        for path in ["registry", "git", "advisory-db"] {
            assert!(!roots.cargo_home.join(path).exists());
        }
        for path in [
            "bin/tool",
            "config.toml",
            "credentials.toml",
            ".crates.toml",
            ".package-cache",
            ".package-cache-mutate",
        ] {
            assert!(roots.cargo_home.join(path).exists());
        }
    }

    #[test]
    fn cargo_in_use_during_scan_or_execution_is_not_removed() {
        let (_temp, roots) = fixture();
        file(&roots.cargo_home.join("registry/src/package/source"));
        let options = opts(&["--cargo-cache"]);
        let locks = CargoLocks::acquire(&roots.cargo_home).unwrap();
        assert!(Plan::build(&roots, &options).unwrap().entries.is_empty());
        drop(locks);
        let plan = Plan::build(&roots, &options).unwrap();
        let _locks = CargoLocks::acquire(&roots.cargo_home).unwrap();
        assert_eq!(plan.execute(&roots, &options), 1);
        assert!(
            roots
                .cargo_home
                .join("registry/src/package/source")
                .exists()
        );
    }

    #[test]
    fn maintenance_is_preserved_if_process_inspection_is_unavailable() {
        let (_temp, mut roots) = fixture();
        file(&roots.cargo_home.join("registry/data"));
        roots.proc = roots.proc.join("missing");
        let plan = Plan::build(&roots, &opts(&["--cargo-cache"])).unwrap();
        assert!(plan.entries.is_empty());
        assert!(!plan.notices.is_empty());
    }
}
