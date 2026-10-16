use clap::{Parser, ValueEnum};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum EngineChoice {
    #[default]
    Auto,
    Podman,
    Docker,
}

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Linux 清理工具：cloacina -a 全部清理，cloacina -an 预览全部",
    after_help = "容器、镜像和未被引用的卷分别选择：全部删除 / 逐个确认 / 跳过。\n卷包含命名卷和匿名卷；确认删除会永久移除数据，仍被容器引用的卷保留。\n默认保留 ~/.cache/gitstatus、Zsh 历史、Claude memory 和容器网络。\n文件清理会删除旧 shell 配置和 .venvs；首次在其他机器运行请先用 -n 查看。"
)]
pub struct Options {
    /// 全部清理：默认项目 + 深度清理 + 系统日志；容器、镜像和卷仍交互确认
    #[arg(short = 'a', long, conflicts_with = "containers_only")]
    pub all: bool,
    /// 只预览，不删除文件、不执行清理命令、不询问
    #[arg(short = 'n', long)]
    pub dry_run: bool,
    /// 加上旧版二进制、Cargo 缓存、共享内存临时文件与包管理器缓存
    #[arg(long, conflicts_with = "containers_only")]
    pub deep: bool,
    /// 加上系统日志（需要 root）
    #[arg(long, conflicts_with = "containers_only")]
    pub system: bool,
    /// 只清理容器、镜像和未被引用的卷（兼容原脚本参数）
    #[arg(long, visible_alias = "containers", conflicts_with = "skip_containers")]
    pub containers_only: bool,
    /// 跳过容器、镜像和卷
    #[arg(long)]
    pub skip_containers: bool,
    /// auto 检测本机已安装的 Podman 和 Docker
    #[arg(long, value_enum, default_value = "auto")]
    pub engine: EngineChoice,
    /// 只清理 /dev/shm 过期且未被使用的临时文件，可与 --old-versions、--cargo-cache 组合
    #[arg(long, conflicts_with_all = ["all", "deep", "system", "containers_only"])]
    pub memory: bool,
    /// 只清理工具旧版本，保留当前版本和运行中的版本
    #[arg(long, conflicts_with_all = ["all", "deep", "system", "containers_only"])]
    pub old_versions: bool,
    /// 只清理 Cargo 依赖缓存，保留工具、配置、凭据与锁文件
    #[arg(long, conflicts_with_all = ["all", "deep", "system", "containers_only"])]
    pub cargo_cache: bool,
    /// /dev/shm 临时文件保留天数
    #[arg(long, default_value_t = 1)]
    pub shm_days: u64,
    /// AI 会话历史保留天数
    #[arg(short = 'd', long, default_value_t = 14)]
    pub keep_days: u64,
    /// /tmp 条目保留天数
    #[arg(long, default_value_t = 3)]
    pub tmp_days: u64,
    /// journal 保留天数
    #[arg(long, default_value_t = 14)]
    pub journal_days: u64,
    /// 额外保留路径；相对路径以家目录为基准，可重复指定
    #[arg(long, value_name = "PATH")]
    pub keep: Vec<PathBuf>,
}

impl Options {
    pub fn targeted(&self) -> bool {
        self.memory || self.old_versions || self.cargo_cache
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_all_preview_and_legacy_flags() {
        let opts = Options::try_parse_from(["cloacina", "-an"]).unwrap();
        assert!(opts.all && opts.dry_run);
        let opts = Options::try_parse_from(["cloacina", "--deep", "--system"]).unwrap();
        assert!(opts.deep && opts.system);
    }

    #[test]
    fn only_conflicts_with_other_scopes() {
        for flag in ["--all", "--deep", "--system", "--skip-containers"] {
            assert!(Options::try_parse_from(["cloacina", "--containers-only", flag]).is_err());
        }
        assert!(Options::try_parse_from(["cloacina", "-d", "-1"]).is_err());
    }

    #[test]
    fn maintenance_scopes_combine_and_do_not_mix_with_broader_scopes() {
        let options = Options::try_parse_from([
            "cloacina",
            "--memory",
            "--old-versions",
            "--cargo-cache",
            "-n",
        ])
        .unwrap();
        assert!(options.targeted() && options.dry_run);
        assert_eq!(options.shm_days, 1);
        for scope in ["--memory", "--old-versions", "--cargo-cache"] {
            for broad in ["-a", "--deep", "--system", "--containers"] {
                assert!(Options::try_parse_from(["cloacina", scope, broad]).is_err());
            }
        }
    }
}
