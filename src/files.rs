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
            home,
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

#[derive(Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub section: &'static str,
    pub bytes: u64,
    identity: Identity,
    temporary: bool,
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
        };
        planner.basic(options);
        if options.deep || options.all {
            planner.old_binaries();
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
                bytes += entry.bytes;
                continue;
            }
            match delete_entry(entry, roots, options.tmp_days) {
                Ok(()) => {
                    println!(
                        "  已删除  {}  ({})",
                        entry.path.display(),
                        human(entry.bytes)
                    );
                    done += 1;
                    bytes += entry.bytes;
                }
                Err(error) => {
                    eprintln!("  未删除 {}：{error:#}", entry.path.display());
                    errors += 1;
                }
            }
        }
        println!(
            "\n文件{}：{done} 项，估算 {}。",
            if options.dry_run { "预演" } else { "清理" },
            human(bytes)
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
        || name.starts_with("ssh-")
        || name.ends_with(".sock")
        || name.ends_with(".lock")
        || matches!(
            name.as_ref(),
            "claude-0" | ".X11-unix" | ".ICE-unix" | ".font-unix" | ".XIM-unix" | ".Test-unix"
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

    fn add(&mut self, path: PathBuf, section: &'static str, temporary: bool) {
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
                    self.add(child, section, temporary);
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
                    temporary,
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
                false,
            );
        }
        for path in self.children(&self.roots.home) {
            let name = path.file_name().unwrap().to_string_lossy();
            if name.starts_with(".zcompdump") || name.starts_with(".claude.json.tmp.") {
                self.add(path, "家目录指定文件与虚拟环境", false);
            }
        }
        let cache = self.roots.home.join(".cache");
        if fs::symlink_metadata(&cache).is_ok_and(|m| m.file_type().is_symlink()) {
            self.plan
                .notices
                .push(format!("{} 是符号链接，保留目标目录", cache.display()));
        }
        for path in self.children(&cache) {
            self.add(path, "家目录缓存（保留 gitstatus）", false);
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
            self.add(self.roots.home.join(name), "可重建状态与快照", false);
        }
        for path in self.children(&self.roots.home.join(".codex")) {
            if path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("logs_2.sqlite")
            {
                self.add(path, "可重建状态与快照", false);
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
            self.add(path, "过期临时文件（保留活跃端点及近期内容）", true);
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
                self.add(path, section, false);
            }
        }
    }

    fn old_binaries(&mut self) {
        let standalone = self.roots.home.join(".codex/packages/standalone");
        if let (Ok(current), Ok(releases)) = (
            fs::canonicalize(standalone.join("current")),
            fs::canonicalize(standalone.join("releases")),
        ) {
            if current.starts_with(&releases) {
                for release in self.children(&standalone.join("releases")) {
                    if fs::canonicalize(&release).is_ok_and(|p| p != current) {
                        self.add(release, "旧版本二进制", false);
                    }
                }
            }
        }
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
                        self.add(path, "旧版本二进制", false);
                    }
                }
                let current_cli = format!("code-{}", current.trim_start_matches("Stable-"));
                for path in self.children(&server) {
                    let name = path.file_name().unwrap().to_string_lossy();
                    if name.starts_with("code-") && name != current_cli {
                        self.add(path, "旧版本二进制", false);
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

fn delete_entry(entry: &Entry, roots: &Roots, tmp_days: u64) -> Result<()> {
    no_symlink_parents(&entry.path)?;
    if contains_mount(&entry.path, &mount_points()?) {
        bail!("路径包含挂载点");
    }
    let meta = fs::symlink_metadata(&entry.path)?;
    if Identity::from(&meta) != entry.identity {
        bail!("路径在扫描后发生变化，请重新扫描");
    }
    if entry.temporary
        && !temporary_tree_is_old(
            &entry.path,
            roots.uid,
            tmp_days,
            SystemTime::now(),
            meta.dev(),
        )?
    {
        bail!("临时目录已有近期内容或活跃端点");
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
            uid: fs::metadata(temp.path()).unwrap().uid(),
            mounts: vec![],
        };
        for path in [&roots.home, &roots.tmp, &roots.logs] {
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
}
