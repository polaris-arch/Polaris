---
status: superseded
updated: 2026-10-05
area: polaris
tags: [archive, source, diagnostics, repository-consolidation]
---

# Branch diagnostic source archive

这些文件保存旧分支中未被当前 main 消费的诊断资产与历史说明。原内容、版本固定值、注释和原路径布局逐字保留；`archive-manifest.json` 给出 source SHA、Git blob、原路径、归档路径、SHA256 和字节数。不要对归档内容批量格式化。归档里的历史 frontmatter、成功记录和执行命令只描述原候选，不更新为本次结果。

| 来源 | 归档目录 | 处置 |
|---|---|---|
| `a0e5eebced9dc4e9f555f028518cc1484f892616` | `c4-mac-masque-ci-20260929/` | 6 个 Mac MASQUE 诊断源码及原 branch-only workflow；原 `.github/` 布局完整保留 |
| `c568183459d1427cd1b12f3787f89b857761a14a` | `windows-source-diagnostic-20261001/` | 固定候选、provider 和 manifest 的原 Windows source diagnostic `package.yml` |
| `9929a38f19e31ce4374acbb5940bd2985df4a8c0` | `g-source-native-sol61-20260930/` | 原 Linux native/source harness 文档与真实 Go provisioning/BuildID fixture |

GitHub Actions 不从 `docs/` 加载这些 workflow；没有新增或替换 `.github/workflows/` 的活动入口。C4 的旧 sing-box/Go/module pins 继续是诊断候选的历史来源，正式 PC/Android producer 仍消费当前统一 source graph。Windows source 修复本体已在 main 的 `633eb5de` 与 `caf0cf50`，历史 workflow 不能替换当前 package 任务。

G 文档的“dependencyPatches 为空”等描述属于 source freeze 前的历史状态。当前 provider、Android consumer、harness、R metadata/receipt dictionary 已在 main 消费并进一步收紧，参见 `13fad86f`、`833d64cc`、`2c77019a`、`6eaea5ba`。这里保留原文以恢复来源，不把旧图、旧 receipt schema 或未审 native 入口重新启用。

## 可执行的独立 fixture

原 fixture 的真实 Go module replay、ELF BuildID、完整输出 hash、新增 compiler input 拒绝和 patch hash 拒绝逻辑适配到 `scripts/core-source-provision.optin.test.py`。它不注册默认 gate；未设置开关时仅报告一个 skipped test，不创建仓库、不调用 Go。

需要独立验证时，显式提供与当前 source manifest `goVersion` 相同的 Go 可执行文件：

```bash
POLARIS_RUN_SOURCE_PROVISION_FIXTURE=1 \
POLARIS_SOURCE_FIXTURE_GO=/absolute/path/to/pinned/go \
POLARIS_NO_KERNEL_RUN=1 \
python3 scripts/core-source-provision.optin.test.py
```

fixture 在 `~/Code/` 下创建并清理临时 Git 仓库，以 `GOPROXY=off`、`GOSUMDB=off`、`GOTOOLCHAIN=local` 构建一个只导入标准库与本地 fixture module 的小 ELF。只读验证 ELF 元数据，不执行 ELF，不构建 Polaris/sing-box，不验证产品 native/network 行为。父流程已使用本机现有 pinned Go1.25.5 显式运行一次：1项通过、exit0（2.449s）。只生成并静态读取 tiny fixture ELF，没有执行 ELF、Polaris/sing-box 或网络/设备测试；原作者“未运行”收据保留，后继见本次统一验证记录。

## 恢复原来源

217 旧 branch heads 的完整出处见 `docs/development/repository-consolidation-20261005/branch-disposition.json`。持久已验证 bundle 位于 `~/docs/polaris/assets/repository-consolidation-2026-10-05/all-refs-before.bundle`（工作副本 `/var/tmp/polaris-unify-20261004/all-refs-before.bundle`），SHA256 为 `740f802d7ae26f2d8b39a307f8915193ec2f2b99a7c56d25b7396a745c24d385`；未提交内容另有父流程的 `dirty-snapshots` 原件。

先检查 bundle，再在新的 `~/Code/<restore-name>` checkout 恢复历史 source SHA：

```bash
sha256sum /var/tmp/polaris-unify-20261004/all-refs-before.bundle
git bundle verify /var/tmp/polaris-unify-20261004/all-refs-before.bundle
git clone /var/tmp/polaris-unify-20261004/all-refs-before.bundle ~/Code/polaris-history-restore-20261005
git -C ~/Code/polaris-history-restore-20261005 switch --detach a0e5eebced9dc4e9f555f028518cc1484f892616
```

在该历史 checkout 对照 `archive-manifest.json` 的 `source_path` 和 `source_blob` 读取原文件。诊断源码的恢复不代表运行授权、正式发布输入或新设备验收；待执行 PC/Android/iOS 工作仍按统一规划单独验收。
