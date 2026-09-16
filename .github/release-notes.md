Rust 版 Linux 清理工具，下载对应架构的单个静态二进制即可运行。

- `cleanup -a`：全部清理；`cleanup -an`：预览全部。
- Podman / Docker 容器和镜像分别选择全部删除、逐个确认或跳过。
- 保留 `~/.cache/gitstatus`、Claude 记忆、当前工具版本、容器卷和网络。
- 兼容原脚本参数，支持额外 `--keep PATH`。

文件清理包含旧 shell 配置和 `.venvs`，首次在另一台机器上运行请先使用 `-an`。

下载 `cleanup-linux-x86_64` 或 `cleanup-linux-aarch64`，核对 `SHA256SUMS` 后安装到 PATH 中。
