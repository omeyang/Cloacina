Cloacina：以古罗马排水与净化女神命名的 Rust Linux 清理工具。项目由 cleanup 更名为 Cloacina，命令为 `cloacina`，仓库及下载包均公开。

- `cloacina -a`：全部清理；`cloacina -an`：预览全部。
- Podman / Docker 容器和镜像分别选择全部删除、逐个确认或跳过。
- 保留 `~/.cache/gitstatus`、Claude 记忆、当前工具版本、容器卷和网络。
- 兼容原脚本参数，支持额外 `--keep PATH`。
- GitHub Actions 在 x86_64 / ARM64 原生 runner 上测试、构建并发布；CI 增加隔离 Podman 集成测试。

文件清理包含旧 shell 配置和 `.venvs`，首次在另一台机器上运行请先使用 `-an`。

下载 `cloacina-linux-x86_64` 或 `cloacina-linux-aarch64`，核对 `SHA256SUMS` 后安装到 PATH 中。
