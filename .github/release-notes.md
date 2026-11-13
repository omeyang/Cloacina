Cloacina v0.4.0 增加按机器配置的容器资源白名单，保护常驻服务、基础镜像和数据卷。

- 支持 `container <name>`、`container-label <key=value>`、`image <repo:tag|id>`、`volume <name>` 四类规则，容器名、label、镜像标签和卷名精确匹配；镜像还支持完整 ID 和至少 12 位十六进制前缀。
- 配置路径依次取 `--whitelist PATH`、`CLOACINA_WHITELIST`、`~/.config/cloacina/whitelist.conf`；文件不存在时沿用原有清理行为。`--no-whitelist` 可临时忽略白名单。
- 命中的资源从删除候选中排除，即使选择“全部删除”也会保留；受保护容器引用的镜像及命名卷、匿名卷自动保留，镜像任一标签命中会保留同一 ID 的全部标签。
- 预览和正式运行均显示白名单保留数量及明细；未匹配资源只提示，配置无法读取或格式错误会报错退出，保护依赖检查失败时跳过该引擎清理。
- 补充配置文档和回归测试，覆盖路径优先级、资源匹配、依赖保护、预览、全部删除和临时禁用；隔离 Podman 集成测试验证实际资源保留及宿主存储不受影响。

例如，在 `~/.config/cloacina/whitelist.conf` 中写入：

```text
container rapidocr-web
container-label PODMAN_SYSTEMD_UNIT=rapidocr-web.service
image localhost/rapidocr-web:latest
volume rapidocr-data
```

运行 `cloacina -an` 查看保留明细和清理候选。label 按键和值精确匹配，不支持 glob；多个 systemd 单元可各写一行。原有文件路径保护 `--keep PATH` 和默认 CoStrict 数据保留规则继续生效。

提供 x86_64、ARM64 的静态 musl 二进制，目标机器无需安装 Rust。下载 `cloacina-linux-x86_64` 或 `cloacina-linux-aarch64`，核对 `SHA256SUMS` 后安装到 PATH 中。
