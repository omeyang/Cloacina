Cloacina v0.3.1 修复 CoStrict 数据目录被误清理的问题。

- 将 `~/.costrict` 移出家目录删除清单，并加入默认保留路径；root 用户的 `/root/.costrict` 及其全部子目录与文件都会保留，无需额外传入 `--keep`。
- 保留规则适用于普通清理、`--deep`、`-a` 和各独立清理类别；预览中也不再将该目录列为删除候选。
- 新增回归测试，覆盖各清理模式、预览与实际删除、嵌套文件、隐藏文件、空目录、符号链接，以及将 CoStrict 目录用作 `CARGO_HOME` 的情况。

提供 x86_64、ARM64 的静态 musl 二进制，目标机器无需安装 Rust。下载 `cloacina-linux-x86_64` 或 `cloacina-linux-aarch64`，核对 `SHA256SUMS` 后安装到 PATH 中。
