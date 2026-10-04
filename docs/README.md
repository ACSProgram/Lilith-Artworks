# Lilith Artworks 文档

## 当前文档

- [AI 阅读引导](architecture/ai-reading-guide.md)
- [系统架构](architecture/overview.md)
- [当前任务交接](planning/current-handoff.md)
- [待办清单](planning/todo.md)
- [规划归档](planning/archive/README.md)
- [Artwork 树模块](modules/library.md)
- [历史与增量备份模块](modules/history-and-backup.md)
- [成品与真实性模块](modules/authenticity.md)
- [素材板模块](modules/pin-board.md)
- [验证策略](guides/validation.md)
- [可靠性压力测试报告](guides/stress-test-report.md)
- [发行政策](guides/release-policy.md)

## 分工

- `architecture/`：跨模块的分层、生命周期、存储布局与运行锁等长期有效的约束。
- `modules/`：各领域模块的上下文入口、契约（DTO / 命令 / 表结构）与领域行为。
- `guides/`：验证与发布流程。
- `planning/current-handoff.md`：当前批次的执行状态与人工验收结果，只写"现在"。
- `planning/todo.md`：未完成、未验证或待决策事项的唯一清单。
- `planning/archive/`：已完成或被替代的规划与批次记录，不再作为执行依据。

架构与指南文档只记录已经确定并持续有效的约束，不保留批次叙事与历史沿革。
