---
status: active
updated: 2026-10-05
area: polaris
tags: [repository, consolidation, branch-audit]
---

# Branch source disposition

固定审计 main `0713990a2cda8cbf476709adabbdefe7e2171034`，217 本地 branch names / 203 unique tips。逐支来源 SHA、覆盖锚、归档锚与恢复 ref 见 `branch-disposition.json`。这是源码销账；未将旧 fork 的中间版本重新覆盖安全修正，也未用 ours merge 制造祖先关系。

| 处置 | branch names |
|---|---:|
| ancestor_integrated | 15 |
| new_historical_diagnostic_archive | 1 |
| patch_equivalent_integrated | 36 |
| ios_source_ported_with_historical_guard | 10 |
| squash_integrated_or_superseded | 151 |
| superseded_fix_with_new_diagnostic_archive | 1 |
| superseded_production_with_new_mock_regression | 1 |
| superseded_source_with_new_fixture_and_archive | 2 |

207 非 iOS branch names 中，15 个已在 main 祖先链，36 个全部非 merge 增量 patch-equivalent；其余主题经 mobile/multi-TS 聚合及后续更强实现处置。10 个 iOS branch names / 9 tips 由最新 `1490dabf48b2b5f2987501bed183da58f56dd25d` 的139路径来源语义迁移，退出/check边界与严格检查卫生使最终owner范围为144路径。sourceKey `ea3b09594eb1abf685923907f57b2f54bc85966ca1ac32cf653e3b3ffd29390b`，Astra xhigh有限核心结论 `PASS_FOR_LIMITED_SOURCE_INTEGRATION`；当前PC/Android能力保留。历史六片缺monitor隔离仍是P1，默认Apple final在effects前拒绝未迁移输入；真实provider/9+2来源/新Framework/App列入统一后续任务，不能将本次源码合流当成运行验收。

## 新消费资产

- C4：7 个历史私有 Mac MASQUE 诊断文件逐字归档；固定旧 core pin 和 branch-only workflow 属历史证据，docs 下模板不激活 CI。
- Windows：原 104 行固定候选源诊断 package workflow 逐字归档；修复本体 `633eb5de`/`caf0cf50` 已集成，不能替换当前正式 package workflow。
- G/R：实际 harness 和 source provider 已集成并升级；补保存旧文档/原真实 Go fixture，新增显式 opt-in tiny-Go BuildID fixture。旧文档的“依赖图为空”是历史状态，不是当前图。
- PC：custody 已重放；从 `5d8e83b5` 保留字符串看似 capacity error 但真实类型为普通 rejection 的 pure mock 反例。

D1 release-risk 和 Windows file identity 两个远端诊断分支的业务和 gate 已被 main 消费；managed CONNECT 后续有跨平台 socket 正规化。SAF acceptance 私有 package ID 由正式 `.debug` 隔离机制替代。旧 tun_stack、BootstrapConfig、activity XML 和下划线 tests 路径是旧模型/脚手架/改名，不能据路径缺失复活。

## 恢复与验证

已验证 bundle：`/home/sway/docs/polaris/assets/repository-consolidation-2026-10-05/all-refs-before.bundle`（17583459 bytes）。SHA256 `740f802d7ae26f2d8b39a307f8915193ec2f2b99a7c56d25b7396a745c24d385`。每支 JSON restore_anchor 指向原 `refs/heads/<branch>`，所有 refs 都在 bundle。

```bash
sha256sum /home/sway/docs/polaris/assets/repository-consolidation-2026-10-05/all-refs-before.bundle
git bundle verify /home/sway/docs/polaris/assets/repository-consolidation-2026-10-05/all-refs-before.bundle
git bundle list-heads /home/sway/docs/polaris/assets/repository-consolidation-2026-10-05/all-refs-before.bundle
```

恢复时在新 `~/Code/<restore-name>` checkout 从 bundle 建仓，再读取 source SHA；不要覆盖最终 main。归档逐文件 SHA256/原 blob ID 在 archive-manifest.json。branch bundle 不包含未提交修改：11 个 dirty worktree 原件由父流程另存 dirty-snapshots，另有专项语义核查。

本审计只读 refs/source/status/diff，没有产品构建、真实 sing-box 或原生网络运行。源码统一不会把待执行真机/native/统一 iOS framework 核验算作已通过。

## 本批最终检查与未完成实验

统一Rust lib2478通过、0失败、14原有ignored；UI全量5388通过、0失败、62原有skipped；Node224通过、0失败、1原有skipped。workspace全target strictClippy、fmt、Vite production build、Android source bridge35与IPC169调用源检查通过。Rust仅源码测试使用空bundle资源列表，8条缺真实打包资源的原失败日志保留，不冒签新产物。未运行真实内核、设备或新Apple构建。

11脏树原件均另行保全；mobile/UI364路径逐项归账（118残差无需恢复旧产品增量），26记录/23去重路径与iOS叠加已由有限核心复核确认保留非iOS语义。helper-exact的9源文件与25文件tracked.patch保存于未激活源码归档，H2未完成任务保留；普通Close/退出不晋升Exact/NoOwner/managed。完整all-refs bundle包含370条catalog及refs/stash，旧UI非branch-tip HEAD可恢复，不以仅heads备份替代。
