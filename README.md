# Cloacina

[![CI](https://github.com/omeyang/Cloacina/actions/workflows/ci.yml/badge.svg)](https://github.com/omeyang/Cloacina/actions/workflows/ci.yml)
[![Release](https://github.com/omeyang/Cloacina/actions/workflows/release.yml/badge.svg)](https://github.com/omeyang/Cloacina/actions/workflows/release.yml)
[![Latest release](https://img.shields.io/github/v/release/omeyang/Cloacina)](https://github.com/omeyang/Cloacina/releases/latest)

名字来自古罗马女神 **Cloacina**：她与排水、净化相关，守护罗马的 Cloaca Maxima。借用这一典故，为 Linux 清走积存的文件、缓存和容器资源。[典故来源：Platner & Ashby《古罗马地形辞典》](https://penelope.uchicago.edu/Thayer/E/Gazetteer/Places/Europe/Italy/Lazio/Roma/Rome/_Texts/PLATOP%2A/Sacrum_Cloacinae.html)

用 Rust 编写的 Linux 清理工具。`cloacina -a` 一次启用全部清理，容器、镜像和未被引用的卷分别交互选择 **全部删除 / 逐个确认 / 跳过**。

提供 **x86_64、ARM64（aarch64）静态 musl 二进制**，目标机器无需 Rust、Python 或项目运行时。文件扫描和删除由 Rust 直接完成；容器清理调用机器上已有的 Podman / Docker，系统清理按需调用 dnf5 / apt-get / journalctl。

## 快速开始

从 [最新 Release](https://github.com/omeyang/Cloacina/releases/latest) 下载对应架构的二进制，按 [安装与升级](https://github.com/omeyang/Cloacina/wiki/安装与升级) 校验并安装后运行：

```sh
cloacina -an                 # 预览全部清理项，不修改数据
cloacina -a                  # 完整清理；容器、镜像和卷交互确认
cloacina --containers        # 只处理容器、镜像和未被引用的卷
cloacina --skip-containers   # 只清理常规文件
cloacina --memory --old-versions --cargo-cache -n  # 只预览这三类，可去掉 -n 执行
cloacina --help
```

文件清理按清单直接执行，包含 Bash 配置和 `.venvs`；可用 `--keep PATH` 保留所需文件或目录。`~/.cache/gitstatus` 始终保留。`-a` 已包含 `--deep --system`，无需再拼接 `--containers-only`，后者表示“仅容器、镜像和卷”。

卷在容器、镜像处理后重新扫描，包含未被任何容器引用的**命名卷和匿名卷**。卷单独确认，默认跳过；删除会永久移除卷内数据。仍被运行中或已停止容器引用的卷保留，删除时不使用强制参数。`-n` 仅展示当前未被引用的卷，实际删除容器后可能出现更多候选。无交互终端时跳过容器、镜像和卷。仅处理当前用户的本机引擎存储，容器网络保留。

`-a` 和 `--deep` 还包含以下项目；也可以用表中的独立参数只清理指定类别，三个参数可组合，支持 `-n` 和 `--keep PATH`，不同时执行常规文件、容器或系统包清理。

| 参数 | 清理范围与保留规则 |
| --- | --- |
| `--memory` | 清理 `/dev/shm` 中当前用户拥有、过期且未被进程使用的临时文件，释放共享内存。默认保留 1 天，可用 `--shm-days N` 调整。保留 socket、锁文件、信号量、挂载点及近期内容；锁文件所在目录的其他过期内容可单独清理。 |
| `--old-versions` | 清理 Codex standalone、app-server-daemon、Claude 原生安装和 VS Code Server 的旧版本。Codex/Claude 保留当前入口指向的版本及进程正在使用的版本；无法确认当前版本时保留。 |
| `--cargo-cache` | 清理 `${CARGO_HOME:-~/.cargo}` 中的 `registry`、`git`、`advisory-db`，保留 `bin`、配置、凭据、安装记录和缓存锁文件。缓存繁忙时跳过，删除期间持有 Cargo 的缓存锁；下次使用需重新下载依赖或检查数据库。 |

执行前会重新核实进程占用、临时文件时效和 Codex/Claude 当前版本。无法检查进程占用时，上述三类保留。结果分别统计磁盘和共享内存的估算释放量；共享内存清理不终止进程，也不操作内核 `drop_caches`。

## Wiki

完整文档在 [GitHub Wiki](https://github.com/omeyang/Cloacina/wiki)，可直接在线阅读与维护。

| 页面 | 内容 |
| --- | --- |
| [安装与升级](https://github.com/omeyang/Cloacina/wiki/安装与升级) | 二进制下载、SHA-256 校验、安装、升级与跨机器使用 |
| [使用指南](https://github.com/omeyang/Cloacina/wiki/使用指南) | 常用命令、全部参数、保留路径、退出码和旧版迁移 |
| [清理范围与保留规则](https://github.com/omeyang/Cloacina/wiki/清理范围与保留规则) | 家目录、缓存、AI 会话、临时文件与系统清理 |
| [容器与镜像](https://github.com/omeyang/Cloacina/wiki/容器与镜像) | 全部/逐个交互、标签、依赖及本机连接规则 |
| [开发与发布](https://github.com/omeyang/Cloacina/wiki/开发与发布) | 构建、测试、GitHub Actions 和 Wiki 维护 |

## GitHub Actions

- [CI](https://github.com/omeyang/Cloacina/actions/workflows/ci.yml)：x86_64 / ARM64 原生测试与静态构建，另有 x86_64 隔离 Podman 集成测试。
- [Release](https://github.com/omeyang/Cloacina/actions/workflows/release.yml)：推送 `v*` 标签后自动测试、构建并发布双架构二进制与 `SHA256SUMS`。

## License

MIT
