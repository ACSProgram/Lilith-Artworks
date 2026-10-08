# Lilith Artworks 文档

文档按读者分两层：`docs/user/` 面向使用者，只描述外部行为与可靠性保证，不写内部契约；
`architecture/`、`modules/`、`guides/`、`planning/` 面向开发者与代理，只记录当前有效的实现
约束，不写批次叙事与历史沿革。根目录的 `README.md`、`SECURITY.md`、`THIRD_PARTY_NOTICES.md`
同属使用者文档。

## 面向使用者

- [可靠性压力测试报告](user/stress-test-report.md)：自动压力测试证明了什么、覆盖哪些真实
  情况、如何自行运行。
- [README](../README.md)：产品定位、当前状态、使用声明与数据可靠性边界。
- [安全策略](../SECURITY.md)：漏洞报告渠道与安全敏感区域。
- [第三方许可摘要](../THIRD_PARTY_NOTICES.md)：人工维护的主要组件许可与告知义务。

## 面向开发者与代理

入口：

- [AI 阅读引导](architecture/ai-reading-guide.md)：代码任务的第一入口，按问题选择一条路由。
- [系统架构](architecture/overview.md)：分层、应用生命周期、存储布局与运行锁。
- [当前任务交接](planning/current-handoff.md)：当前批次状态与人工验收结果。
- [待办清单](planning/todo.md)：未完成、未验证与待决策事项的唯一清单。

领域模块：

- [Artwork 树模块](modules/library.md)
- [历史与增量备份模块](modules/history-and-backup.md)
- [成品与真实性模块](modules/authenticity.md)
- [素材板模块](modules/pin-board.md)

流程与计划：

- [验证策略](guides/validation.md)：分层验证、压力测试边界与持续集成。
- [发行政策](guides/release-policy.md)：版本、发布门槛与产物。
- [0.3 架构与核心算法改进计划](planning/0.3-architecture-algorithm-plan-2026-10-05.md)：提案，
  尚未实施。
- [规划归档](planning/archive/README.md)：已完成或被替代的规划与批次记录，不再作为执行依据。

## 目录分工

- `architecture/`：跨模块的分层、生命周期、存储布局与运行锁等长期有效的约束。
- `modules/`：各领域模块的上下文入口、契约（DTO / 命令 / 表结构）与领域行为。
- `guides/`：验证与发布流程。
- `planning/current-handoff.md`：当前批次状态与人工验收结果，只写"现在"。
- `planning/todo.md`：未完成、未验证或待决策事项的唯一清单。
- `planning/archive/`：已完成或被替代的规划与批次记录，不再作为执行依据。
