//! 平台差异的唯一入口。
//!
//! 本模块把散落在业务代码里的 `cfg(target_os)` / `cfg(windows)` 分支集中收口：
//! 对外只暴露平台无关命名的接口（见 `docs/guides/platform.md`），各平台的实现
//! 差异（包括「该平台没有这个能力」的显式空实现）全部在模块内部消化。业务代码
//! 禁止再直接写平台分支；新增分支的流程与例外登记表见规范文档。
//!
//! 建模原则：按「业务需要什么」命名接口，不按 Windows 命名；能力缺失必须显式
//! （返回空列表 / `None` / 明确的错误），错误文案跨平台逐字一致。

pub(crate) mod clipboard;
pub(crate) mod path;
pub(crate) mod process;
pub(crate) mod shell;
