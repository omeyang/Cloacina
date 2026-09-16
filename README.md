# Cloacina

[![CI](https://github.com/omeyang/cloacina/actions/workflows/ci.yml/badge.svg)](https://github.com/omeyang/cloacina/actions/workflows/ci.yml)
[![Release](https://github.com/omeyang/cloacina/actions/workflows/release.yml/badge.svg)](https://github.com/omeyang/cloacina/actions/workflows/release.yml)
[![Latest release](https://img.shields.io/github/v/release/omeyang/cloacina)](https://github.com/omeyang/cloacina/releases/latest)

名字来自古罗马女神 **Cloacina**：她与排水、净化相关，守护罗马的 Cloaca Maxima。借用这一典故，为 Linux 清走积存的文件、缓存和容器资源。[典故来源：Platner & Ashby《古罗马地形辞典》](https://penelope.uchicago.edu/Thayer/E/Gazetteer/Places/Europe/Italy/Lazio/Roma/Rome/_Texts/PLATOP%2A/Sacrum_Cloacinae.html)

用 Rust 编写的 Linux 清理工具。`cloacina -a` 一次启用全部清理，容器和镜像分别交互选择 **全部删除 / 逐个确认 / 跳过**。

提供 **x86_64、ARM64（aarch64）静态 musl 二进制**，目标机器无需 Rust、Python 或项目运行时。文件扫描和删除由 Rust 直接完成；容器清理调用机器上已有的 Podman / Docker，系统清理按需调用 dnf5 / apt-get / journalctl。

## 安装

从 [Releases](https://github.com/omeyang/cloacina/releases) 下载与你的 `uname -m` 对应的 `cloacina-linux-x86_64` 或 `cloacina-linux-aarch64`。`SHA256SUMS` 与二进制位于同一发布页。

仓库和发布包均公开，无需 GitHub 账号。下载与你的架构匹配的二进制，核对 SHA-256 后安装：

```sh
(
  set -eu
  cloacina_asset="cloacina-linux-$(uname -m)"
  case "$cloacina_asset" in
    cloacina-linux-x86_64|cloacina-linux-aarch64) ;;
    *) printf '暂不提供当前架构的二进制\n' >&2; exit 1 ;;
  esac
  mkdir -p cloacina-download
  cd cloacina-download
  cloacina_release='https://github.com/omeyang/cloacina/releases/latest/download'
  curl -fL "$cloacina_release/$cloacina_asset" -o "$cloacina_asset"
  curl -fL "$cloacina_release/SHA256SUMS" -o SHA256SUMS
  sha256sum --check --ignore-missing SHA256SUMS
  mkdir -p "$HOME/.local/bin"
  install -m 755 "$cloacina_asset" "$HOME/.local/bin/cloacina"
)
cloacina --version
```

也可以使用 GitHub CLI：

```sh
mkdir -p cloacina-download
gh release download --repo omeyang/cloacina --dir cloacina-download
(
  cd cloacina-download
  sha256sum -c SHA256SUMS
)
mkdir -p "$HOME/.local/bin"
install -m 755 "cloacina-download/cloacina-linux-$(uname -m)" "$HOME/.local/bin/cloacina"
cloacina --version
```

确保 `~/.local/bin` 在 `PATH` 中。二进制也可以直接复制到另一台相同架构的 Linux 机器运行。只支持 64 位 Linux；不适用于 macOS、Windows 或 32 位 ARM。

## 用法

```sh
cloacina -an                 # 预览全部清理项目，不修改数据
cloacina -a                  # 全部清理；容器、镜像仍需交互选择
cloacina                     # 常规文件清理 + 容器/镜像交互
cloacina --containers        # 只清容器和镜像
cloacina --skip-containers   # 只清文件
cloacina --engine docker     # 只询问 Docker 的容器和镜像
cloacina --keep .bashrc --keep .venvs/work
cloacina --help
```

`--deep`、`--system`、`--containers-only`、`-d 30` 继续兼容原脚本。`--all` / `-a` 等于常规清理 + `--deep --system`，不需要再加 `--containers-only`；后者表示“仅容器/镜像”，不能与全部清理组合。

交互菜单中，`1` 删除该类的全部条目，`2` 逐个询问，`3` 或回车跳过。在逐项确认中，`y` 同意、回车拒绝、`q` 跳过本组剩余条目。运行中的容器会在删除时停止。没有交互终端时跳过容器与镜像，文件清理仍按所选参数执行。

## 清理范围

| 范围 | 内容 |
| --- | --- |
| 常规家目录 | `.zcompdump*`、`.bash_history`、`.bash_profile`、`.bash_logout`、`.bashrc`、`.shell.pre-oh-my-zsh`、`.tcshrc`、`.cshrc`、`.ksshrc`、`.venvs` |
| 其他历史杂项 | `.tig_history`、`.lesshst`、`.wget-hsts`、`.viminfo`、`.my-git-sync-cron.sh`、`.roo`、`.costrict`、`.dotnet`、`gobuild-verify.log`、`.claude.json.tmp.*` |
| 缓存 | `~/.cache` 的内容，**保留 `~/.cache/gitstatus`** |
| AI 状态 | 超过 14 天的 Codex 会话和 Claude `.jsonl` 会话；Codex/Claude shell 快照、Claude paste-cache、Codex `logs_2.sqlite*` |
| 临时文件 | `/tmp` 下属于当前有效用户、超过 3 天且内部没有近期内容的条目；单独处理 `/tmp/claude-0` 的过期会话目录 |
| `--deep` | Codex 和 VS Code Server 的旧版本；root 用户执行 `dnf5 clean all` 或 `apt-get clean` |
| `--system` | 超过 1 天的轮转日志；`journalctl --vacuum-time=14d`，需要 root |
| 容器与镜像 | 自动检测已安装的 Podman / Docker，同一镜像的多个标签合并询问 |

**文件清理按照清单直接执行。** 换机器时先运行 `cloacina -an`：清单包含 Bash 配置和整个 `.venvs`，需要保留时使用可重复的 `--keep PATH`。相对保留路径以家目录为基准。`--keep` 只保护文件路径，不改变包管理器、journal 或容器清理。

`--keep-days` 调整 AI 会话保留天数，`--tmp-days` 调整临时文件保留天数，`--journal-days` 调整 journal 保留天数。家目录来自当前进程的 `HOME`；程序不自动提权，`sudo` 后的 `HOME` 可能不同。

## 保留与边界

- 始终保留 `~/.cache/gitstatus`、`.zsh_history`、Claude `memory/`、当前 Codex release、当前 VS Code server 和扩展。
- 不删除容器卷、网络；删除镜像不使用 `--force`，避免删除用户保留的 Podman 容器。被保留容器引用的镜像及标签一并保留并报告未完成。
- 逐个确认仅作用于所选 ID；不自动清理未选择的父镜像。全部模式同样使用明确的 ID，并在有进展时重试父子镜像依赖。
- Podman 固定使用 `--remote=false`。Docker 只接受本机 Unix socket，固定 `--host` 并清除可能改写连接的环境变量；远程 context 和 TCP endpoint 会跳过。
- 不跟随文件清理路径中的目录符号链接。`.cache` 本身若是链接则保留；被选中的普通符号链接只删除链接。
- 跳过挂载点和包含挂载点的目录。删除前复查路径身份；临时目录还会复查所有者、更新时间、socket、FIFO 和锁文件。`/tmp` 本身、`systemd-private-*`、`ssh-*` 等始终保留。
- 退出码 `0` 表示完成或主动跳过，`1` 表示清理失败或选中的资源仍有依赖，`2` 表示参数错误。

静态链接解决 libc 运行时依赖，仍需与机器架构、Linux 内核以及已有容器引擎兼容；它不包含容器引擎本身。

## 构建与验证

需要 Rust 1.85 或更新版本。

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
cargo build --release --locked --target x86_64-unknown-linux-musl
cargo build --release --locked --target aarch64-unknown-linux-musl
```

项目使用 Rust 自带的 `rust-lld` 链接器，无需单独安装 musl-gcc 或跨架构 C 工具链。

## GitHub Actions

- [CI](https://github.com/omeyang/cloacina/actions/workflows/ci.yml)：每次推送到 `main` 或提交 PR，分别在 x86_64、ARM64 原生 runner 上运行格式检查、Clippy、测试、静态构建和二进制启动检查；x86_64 还运行隔离 Podman 集成测试。支持手动触发。
- [Release](https://github.com/omeyang/cloacina/actions/workflows/release.yml)：推送 `v*` 标签时，检查标签与 Cargo 版本一致，测试并构建两种架构，发布 `cloacina-linux-x86_64`、`cloacina-linux-aarch64` 和 `SHA256SUMS`。

隔离 Podman 集成测试（仅在临时存储内创建测试容器和镜像）：

```sh
python3.12 tests/integration_podman.py target/x86_64-unknown-linux-musl/release/cloacina
```

## 从 cleanup 迁移

项目和命令从 v0.2.0 起统一命名为 `cloacina`。原有参数保持兼容；`cloacina -a` 对应原来的 `cleanup -a`。旧版 `v0.1.0` 保留在发布历史中。

## License

MIT
