Cloacina v0.3.0 补齐容器卷、共享内存临时文件、工具旧版本和 Cargo 缓存清理。

- Podman / Docker 新增独立的卷清理步骤，支持全部删除、逐个确认和跳过。删除容器后重新扫描，覆盖未被引用的命名卷和匿名卷；保留仍被容器引用的卷，不强制删除。
- `-a` / `--deep` 新增 `/dev/shm` 过期临时文件清理，默认保留 1 天，可通过 `--shm-days N` 调整。保留进程正在使用的文件、socket、锁文件、信号量和挂载点；锁文件旁的过期构建产物可单独回收。
- 扩展 Codex standalone、app-server-daemon 和 Claude 原生安装的旧版本清理。保留当前入口指向的版本与运行中的版本，删除前重新核实。
- 新增 Cargo 的 `registry`、`git` 和 `advisory-db` 缓存清理，支持 `CARGO_HOME`。删除期间持有 Cargo 缓存锁，保留工具、配置、凭据、安装记录和锁文件。
- 新增可组合的 `--memory --old-versions --cargo-cache`，只处理选定类别，支持 `-n` 和 `--keep PATH`；分别统计磁盘与共享内存的估算释放量。

卷清理默认跳过，确认删除后会永久移除卷内数据。Cargo 缓存清理后，下次使用需重新下载依赖或检查数据库。常规文件清理仍包含旧 shell 配置和 `.venvs`，首次运行请先使用 `cloacina -an` 预览。

提供 x86_64、ARM64 的静态 musl 二进制，目标机器无需安装 Rust。下载 `cloacina-linux-x86_64` 或 `cloacina-linux-aarch64`，核对 `SHA256SUMS` 后安装到 PATH 中。
