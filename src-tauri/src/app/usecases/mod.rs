//! GUI 与 headless 共享的业务入口。
//!
//! 每个函数只依赖 [`CommandContext`](crate::app::command_runtime::CommandContext)
//! 与领域公开服务，不接收 Tauri 的 `State` / `WebviewWindow`。适配器负责解析 DTO、
//! 检查文件选择器授权并把 `CommandError` 映射回既有错误形态。

pub(crate) mod authenticity;
pub(crate) mod backup;
pub(crate) mod cleanup;
pub(crate) mod history;
pub(crate) mod library;
pub(crate) mod pin_board;
pub(crate) mod settings;
pub(crate) mod storage;

use crate::app::command_runtime::{CommandContext, CommandError};

/// 重放清理队列后广播待清理数量。Library、Pin-board 与 Cleanup 三个入口共用同一
/// 口径：计数失败只让事件缺少数量，不改变命令结果。
///
/// 这里写全 `crate::cleanup`：本模块内的 `cleanup` 指的是 use-case 子模块。
pub(crate) fn report_cleanup_queue(context: &CommandContext) {
    let pending = context
        .run_read("count_pending_cleanup", |session| {
            crate::cleanup::list_pending(session.root()).map_err(CommandError::from)
        })
        .map(|entries| entries.len() as u64)
        .ok();
    context.emit_cleanup_queue_changed(pending);
}
