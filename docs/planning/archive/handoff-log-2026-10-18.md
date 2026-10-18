# 2026-10-18 交接归档：0.2 收口与 0.3 开发线建立

本文件归档 2026-10-18 从 `../current-handoff.md` 清出的已完成批次：`0.2.0` 正式版收口，以及
`main` 冻结、`dev` 建立为 0.3 开发线。当前状态改看 `../current-handoff.md`。

## 0.2.0 正式版收口

- **切分提交**：`0be6224 Cut version 0.2.0 and align release infrastructure`。产品代码与
  `0.2.0-rc.4` 完全一致，切分只改版本字段、CHANGELOG 与发布基建：
  - `CHANGELOG.md` 新建带日期小节 `## 0.2.0 - 2026-10-17`，并入 rc.4 小节内容，补入压测、桌面
    验收与已知限制，并记录发布基建修正；顶部补回 `## Unreleased` 暂存段。
  - 五处版本字段、许可清单与 README 版本行同步到 `0.2.0`；`verify-metadata.mjs` 验证通过。
  - release workflow 对稳定标签不再标 prerelease 并更名；verify-metadata 补 Cargo.lock 版本校验。
- **标签**：`v0.2.0` 附注标签已打并推送，指向 `0be6224`；`v0.2.0-rc.3` 未打标签。
- **压力测试**：rc.4 的全套餐压力测试按维护者口径即 0.2.0 正式版的压力测试（产品代码一致）。
  2026-10-17 已在 release 构建上完整重跑并通过（八组便宜分组 + gate_guard 40 用例、C 组五档
  7/7 含 C7、G 组 extreme 档、D/E/F 规模组、P 组基准复测同量级），实测数据在
  `docs/user/stress-test-report.md`，被测版本口径为 `0.2.0`。
- **自动门槛**：Windows CI 已在 rc.4 与切分提交上通过；正式版门槛由 `v0.2.0` 标签触发的 release
  workflow 执行（依赖审计、许可一致性、从标签构建 NSIS 并静默安装检查、SBOM、校验和、attestation、
  草稿 release）。
- **人工门槛**：干净 Windows 用户环境全清单（2026-10-16）与任务调度总控、空闲链路校验及安全批次
  的界面项（2026-10-17，随 rc.4）均已通过；0.2.0 与 rc.4 产品代码一致，验收沿用，记录见
  `handoff-log-2026-10-16.md`。
- **发布口径现状**：GitHub 私密漏洞报告渠道已开启，满足发行政策前置项。Authenticode 签名仍无法
  进行；按 `docs/guides/release-policy.md` 允许未签名发布，发布说明须显著披露未签名状态及
  SmartScreen 影响，签名相关人工门槛项在证书可用前无法执行。

## 分支模型调整

- `main` 固定为 0.2 发布线，停留在 `0be6224`，不再接收 0.3 改动。
- `dev` 从 `0be6224` 创建，作为 0.3 开发线；0.3 按
  `0.3-architecture-algorithm-plan-2026-10-05.md` 分 alpha1（阶段 1–5）与 alpha2（阶段 6–9）
  两批实施。
- 0.3 的日常改动仍遵守版本号滞后策略：不切版本时不改五处版本字段，改动写入 `CHANGELOG.md` 的
  `## Unreleased`；alpha 批次不自动等于公开版本，切版本与打标签由维护者单独决定。
