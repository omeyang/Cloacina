# Cloacina

[![CI](https://github.com/omeyang/Cloacina/actions/workflows/ci.yml/badge.svg)](https://github.com/omeyang/Cloacina/actions/workflows/ci.yml)
[![Release](https://github.com/omeyang/Cloacina/actions/workflows/release.yml/badge.svg)](https://github.com/omeyang/Cloacina/actions/workflows/release.yml)
[![Latest release](https://img.shields.io/github/v/release/omeyang/Cloacina)](https://github.com/omeyang/Cloacina/releases/latest)

名字来自古罗马女神 **Cloacina**：她与排水、净化相关，守护罗马的 Cloaca Maxima。借用这一典故，为 Linux 清走积存的文件、缓存和容器资源。[典故来源：Platner & Ashby《古罗马地形辞典》](https://penelope.uchicago.edu/Thayer/E/Gazetteer/Places/Europe/Italy/Lazio/Roma/Rome/_Texts/PLATOP%2A/Sacrum_Cloacinae.html)

用 Rust 编写的 Linux 清理工具。`cloacina -a` 一次启用全部清理，容器和镜像分别交互选择 **全部删除 / 逐个确认 / 跳过**。

提供 **x86_64、ARM64（aarch64）静态 musl 二进制**，目标机器无需 Rust、Python 或项目运行时。文件扫描和删除由 Rust 直接完成；容器清理调用机器上已有的 Podman / Docker，系统清理按需调用 dnf5 / apt-get / journalctl。

## 快速开始

从 [最新 Release](https://github.com/omeyang/Cloacina/releases/latest) 下载对应架构的二进制，按 [安装与升级](https://github.com/omeyang/Cloacina/wiki/安装与升级) 校验并安装后运行：

```sh
cloacina -an                 # 预览全部清理项，不修改数据
cloacina -a                  # 完整清理；容器和镜像交互确认
cloacina --containers        # 只处理容器和镜像
cloacina --skip-containers   # 只清理常规文件
cloacina --help
```

文件清理按清单直接执行，包含 Bash 配置和 `.venvs`；可用 `--keep PATH` 保留所需文件或目录。`~/.cache/gitstatus` 始终保留。`-a` 已包含 `--deep --system`，无需再拼接 `--containers-only`，后者表示“仅容器和镜像”。

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
