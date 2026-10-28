# 未完成 PC helper exact 草稿

这批 H1 私有快照、H2a cache lease 状态机、H2b POSIX CAS/fsync 账本、H2c owner 目录观察来自 2026-09-29 未提交工作。它尚未接入 StartExact 生产准入，不能解释成 Exact/NoOwner/managed 已交付。此目录不在 Cargo workspace 或 CI 门中，不会替换现行 helper 的 birth/custody 协议。

源码九路径逐字保存在 `source/`；基线及 SHA 在 `manifest.json`。`tracked.patch` 保存该基线上的25个既有文件改动。恢复实验时，在 `~/Code/` 新建基于 manifest baseline 的隔离工作树，先 `git apply --check` 再应用 tracked.patch，将 source 目录按相对路径拷回。原始基线可以从合流持久备份的 all-refs-before.bundle 恢复。不得把该补丁直接覆盖当前 main。

归档 `.gitattributes` 仅对 `tracked.patch` 停用容器空白检查，因为 unified diff 上下文空行保留必要前缀空格；原始字节及 SHA-256 `e1b8377958a78240179f4ddd4b69307ea2f29582cb523a44f03369ecbfa87c56` 必须保持，源码文件仍执行通常空白检查。

后续统一任务：先与当前 exact_start/CoreReceipt 协议核对；明确版本、持久 cache 授权、未启动/构造失败副作用、崩溃 journal 与 owner 目录归属；再按受支持平台完成生命周期和准入闭包及必要复审。原有普通连接/停核行为不因本归档改变。
