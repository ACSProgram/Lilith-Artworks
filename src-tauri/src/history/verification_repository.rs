//! 节点级完整性校验状态：链元数据清单、结论写入与分支派生状态。
//!
//! 本模块只读写 SQLite 图元数据；链的物化、归因与载荷校验在 `backup::restore`。
//! 扫描通过批量写入器提交结论，单节点入口留给只写一个结果的调用方。

use std::{collections::HashMap, path::Path};

use rusqlite::params;

use crate::storage;

use super::{classify_verify_failure, NodeVerifyIssue, VerifyChainNode, VerifyNodeTarget};

/// 全库扫描一次读出所有节点的链元数据，之后沿链走完整个仓库不再查库。
///
/// `delta_path` 取自节点的入边（`history_edges.child_history_id = id`），与
/// `materialization_chain` 的口径一致：它是「本节点的内容还原出父节点」的反向
/// 增量。空串按「没有增量」处理（检查点父节点名下的合法记账），兼容列只在没有
/// 边记录时兜底。
pub(crate) fn verification_inventory(root: &Path) -> Result<Vec<VerifyChainNode>, String> {
    let connection = storage::open(root)?;
    collect_verify_nodes(
        &connection,
        "SELECT node.id, node.parent_id, node.sha256, node.snapshot_path,
                COALESCE(NULLIF(TRIM(edge.delta_path), ''), NULLIF(TRIM(node.delta_path), '')),
                node.created_ms
         FROM history_nodes node
         LEFT JOIN history_edges edge ON edge.child_history_id = node.id
         ORDER BY node.created_ms, node.id",
        &[],
    )
}

/// 单条分支祖先链的节点元数据，字段口径与 [`verification_inventory`] 一致，
/// 只是范围收在该分支 head 到根节点之间。手动重查按这条链走，不读取其它分支。
pub(crate) fn branch_verification_inventory(
    root: &Path,
    branch_id: &str,
) -> Result<Vec<VerifyChainNode>, String> {
    let connection = storage::open(root)?;
    collect_verify_nodes(
        &connection,
        "WITH RECURSIVE ancestors(id) AS (
             SELECT head_history_id FROM branches
             WHERE id = ?1 AND head_history_id IS NOT NULL
             UNION ALL
             SELECT node.parent_id
             FROM history_nodes node JOIN ancestors ON node.id = ancestors.id
             WHERE node.parent_id IS NOT NULL
         )
         SELECT node.id, node.parent_id, node.sha256, node.snapshot_path,
                COALESCE(NULLIF(TRIM(edge.delta_path), ''), NULLIF(TRIM(node.delta_path), '')),
                node.created_ms
         FROM history_nodes node
         JOIN ancestors ON ancestors.id = node.id
         LEFT JOIN history_edges edge ON edge.child_history_id = node.id
         ORDER BY node.created_ms, node.id",
        &[&branch_id],
    )
}

fn collect_verify_nodes(
    connection: &rusqlite::Connection,
    sql: &str,
    params: &[&dyn rusqlite::ToSql],
) -> Result<Vec<VerifyChainNode>, String> {
    let mut statement = connection.prepare(sql).map_err(storage::database_error)?;
    let nodes = statement
        .query_map(params, |row| {
            Ok(VerifyChainNode {
                id: row.get(0)?,
                parent_id: row.get(1)?,
                sha256: row.get(2)?,
                snapshot_path: row.get(3)?,
                delta_path: row.get(4)?,
                created_ms: row.get(5)?,
            })
        })
        .map_err(storage::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)?;
    Ok(nodes)
}

/// 派生待查集合：未进回收站的作品中，尚未校验且没有失败摘要的历史节点，最多
/// 返回 `limit` 条。
///
/// 按 `created_ms` 最旧优先——最久未读的节点正是 bit rot 风险最高的节点。失败
/// 节点（`verify_error` 非空）退出自动队列，直到被改写或用户手动重查。批次大小
/// 由调用方给进 SQL：待查集合在旧仓库上可能有上万条，没必要整份读进内存再截断。
pub(crate) fn list_pending_verify_nodes(
    root: &Path,
    limit: usize,
) -> Result<Vec<VerifyNodeTarget>, String> {
    let connection = storage::open(root)?;
    let mut statement = connection
        .prepare(
            "SELECT node.id
             FROM history_nodes node
             JOIN library_nodes artwork ON artwork.id = node.artwork_id
             WHERE artwork.trashed_ms IS NULL
               AND node.verified_ms IS NULL
               AND node.verify_error IS NULL
             ORDER BY node.created_ms, node.id
             LIMIT ?1",
        )
        .map_err(storage::database_error)?;
    let limit = limit.min(i64::MAX as usize) as i64;
    let targets = statement
        .query_map([limit], |row| {
            Ok(VerifyNodeTarget {
                history_id: row.get(0)?,
            })
        })
        .map_err(storage::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)?;
    Ok(targets)
}

/// 与 [`list_pending_verify_nodes`] 同一口径的只读计数，供设置页展示。
pub(crate) fn count_pending_verify_nodes(root: &Path) -> Result<usize, String> {
    storage::open(root)?
        .query_row(
            "SELECT COUNT(*)
             FROM history_nodes node
             JOIN library_nodes artwork ON artwork.id = node.artwork_id
             WHERE artwork.trashed_ms IS NULL
               AND node.verified_ms IS NULL
               AND node.verify_error IS NULL",
            [],
            |row| row.get(0),
        )
        .map_err(storage::database_error)
}

/// 清除整条分支链的失败摘要，使这些节点重新进入自动队列；已通过校验的状态保留。
pub(crate) fn clear_branch_verify_errors(root: &Path, branch_id: &str) -> Result<(), String> {
    let connection = storage::open(root)?;
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM branches WHERE id = ?1)",
            [branch_id],
            |row| row.get(0),
        )
        .map_err(storage::database_error)?;
    if !exists {
        return Err("找不到分支".into());
    }
    connection
        .execute(
            "WITH RECURSIVE ancestors(id) AS (
               SELECT head_history_id FROM branches
               WHERE id = ?1 AND head_history_id IS NOT NULL
               UNION ALL
               SELECT node.parent_id
               FROM history_nodes node JOIN ancestors ON node.id = ancestors.id
               WHERE node.parent_id IS NOT NULL
             )
             UPDATE history_nodes SET verify_error = NULL
             WHERE id IN (SELECT id FROM ancestors)",
            [branch_id],
        )
        .map_err(storage::database_error)?;
    Ok(())
}

/// 每次批量提交的节点数。全库扫描上千节点时，逐条新建连接与隐式事务会把成本
/// 压在连接与提交上；按批提交只保留一个连接。
const VERIFICATION_BATCH_SIZE: usize = 128;

struct PendingVerification {
    history_id: String,
    verified_ms: Option<i64>,
    verify_error: Option<String>,
}

/// 节点级校验状态的批量写入器。
///
/// 记录先累积，每 [`VERIFICATION_BATCH_SIZE`] 条或调用 [`Self::finish`] 时在一个
/// 事务里提交。扫描被取消时写入器随作用域丢弃，未提交的一批不会落库；已经提交的
/// 批次保留，与「已处理的批次仍然写入」的既有口径一致。
pub(crate) struct VerificationWriter {
    connection: rusqlite::Connection,
    pending: Vec<PendingVerification>,
}

impl VerificationWriter {
    pub(crate) fn open(root: &Path) -> Result<Self, String> {
        Ok(Self {
            connection: storage::open(root)?,
            pending: Vec::with_capacity(VERIFICATION_BATCH_SIZE),
        })
    }

    pub(crate) fn record_verified(
        &mut self,
        history_id: &str,
        verified_ms: i64,
    ) -> Result<(), String> {
        self.push(PendingVerification {
            history_id: history_id.to_owned(),
            verified_ms: Some(verified_ms),
            verify_error: None,
        })
    }

    pub(crate) fn record_error(&mut self, history_id: &str, error: &str) -> Result<(), String> {
        self.push(PendingVerification {
            history_id: history_id.to_owned(),
            verified_ms: None,
            verify_error: Some(error.to_owned()),
        })
    }

    fn push(&mut self, entry: PendingVerification) -> Result<(), String> {
        self.pending.push(entry);
        if self.pending.len() >= VERIFICATION_BATCH_SIZE {
            self.commit_pending()?;
        }
        Ok(())
    }

    fn commit_pending(&mut self) -> Result<(), String> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let transaction = self
            .connection
            .transaction()
            .map_err(storage::database_error)?;
        for entry in &self.pending {
            let changed = transaction
                .execute(
                    "UPDATE history_nodes SET verified_ms = ?2, verify_error = ?3 WHERE id = ?1",
                    params![entry.history_id, entry.verified_ms, entry.verify_error],
                )
                .map_err(storage::database_error)?;
            if changed == 0 {
                return Err(format!("找不到历史节点 {}", entry.history_id));
            }
        }
        transaction.commit().map_err(storage::database_error)?;
        self.pending.clear();
        Ok(())
    }

    /// 提交最后一批。取消路径不会调用它，因此未完成批次不会落库。
    pub(crate) fn finish(mut self) -> Result<(), String> {
        self.commit_pending()
    }
}

/// 记录一个节点的校验通过；`verify_error` 同时清空。只由测试直接调用，生产路径
/// 走 [`VerificationWriter`]。
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn mark_node_verified(
    root: &Path,
    history_id: &str,
    verified_ms: i64,
) -> Result<(), String> {
    record_node_verification(root, history_id, Some(verified_ms), None)
}

/// 记录一个节点的校验失败；失败节点退出自动队列，直到被改写或手动重查。只由测试
/// 直接调用，生产路径走 [`VerificationWriter`]。
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn mark_node_verify_error(
    root: &Path,
    history_id: &str,
    error: &str,
) -> Result<(), String> {
    record_node_verification(root, history_id, None, Some(error))
}

/// 写入节点级校验状态。调用方都在共享运行锁内，节点不可能在校验与写回之间消失，
/// 因此 0 行受影响只可能是 id 不属于本仓库，按错误处理而不是静默忽略。
#[cfg_attr(not(test), allow(dead_code))]
fn record_node_verification(
    root: &Path,
    history_id: &str,
    verified_ms: Option<i64>,
    error: Option<&str>,
) -> Result<(), String> {
    let changed = storage::open(root)?
        .execute(
            "UPDATE history_nodes SET verified_ms = ?2, verify_error = ?3 WHERE id = ?1",
            params![history_id, verified_ms, error],
        )
        .map_err(storage::database_error)?;
    if changed == 0 {
        return Err(format!("找不到历史节点 {history_id}"));
    }
    Ok(())
}

pub(crate) struct NodeVerificationState {
    parent_id: Option<String>,
    verify_error: Option<String>,
    verified_ms: Option<i64>,
}

pub(crate) fn node_verification_states(
    connection: &rusqlite::Connection,
    artwork_id: &str,
) -> Result<HashMap<String, NodeVerificationState>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, parent_id, verify_error, verified_ms
             FROM history_nodes WHERE artwork_id = ?1",
        )
        .map_err(storage::database_error)?;
    let states = statement
        .query_map([artwork_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                NodeVerificationState {
                    parent_id: row.get(1)?,
                    verify_error: row.get(2)?,
                    verified_ms: row.get(3)?,
                },
            ))
        })
        .map_err(storage::database_error)?
        .collect::<Result<HashMap<_, _>, _>>()
        .map_err(storage::database_error)?;
    Ok(states)
}

/// 从 head 沿父链派生的分支校验状态。
pub(crate) struct BranchVerification {
    /// 链上第一个「有问题」节点的文案；全部正常时为空。
    pub(crate) error: Option<String>,
    /// 链上确认出问题的节点数（丢失/损坏）。摘要只说「无法读取」，计数让界面能说清
    /// 「有几个」。
    pub(crate) failure_count: u32,
    /// 链上「无法检查」的节点数。它们自己的文件未必坏，只是链被更深的坏节点挡住。
    pub(crate) unverifiable_count: u32,
    /// 链上节点里最严重的一类，供界面统一配色。
    pub(crate) issue: Option<NodeVerifyIssue>,
    /// 链上全部节点都通过校验时取最旧的通过时间。
    pub(crate) verified_ms: Option<i64>,
}

/// 从 head 沿父链派生分支的第三态：任一节点 `verify_error` 非空即有问题；否则
/// 全部节点都有 `verified_ms` 时取最旧的通过时间，任一节点未验证则为 NULL。
pub(crate) fn derive_branch_verification(
    head_history_id: Option<&str>,
    nodes: &HashMap<String, NodeVerificationState>,
) -> BranchVerification {
    let Some(mut current) = head_history_id else {
        return BranchVerification {
            error: None,
            failure_count: 0,
            unverifiable_count: 0,
            issue: None,
            verified_ms: None,
        };
    };
    let mut error = None;
    let mut blocked_error = None;
    let mut failure_count = 0_u32;
    let mut unverifiable_count = 0_u32;
    let mut issue: Option<NodeVerifyIssue> = None;
    let mut oldest_verified: Option<i64> = None;
    let mut all_verified = true;
    let mut seen = false;
    while let Some(state) = nodes.get(current) {
        seen = true;
        if let Some(message) = state.verify_error.as_deref() {
            let classified = classify_verify_failure(message);
            // 「无法检查」不是这个节点自己的问题，单独计数：把它算进「无法读取」
            // 会让用户以为坏了 N 个，而实际上可能有 N-1 个只是被别人挡住。
            if classified == NodeVerifyIssue::Unverifiable {
                unverifiable_count += 1;
                blocked_error.get_or_insert_with(|| message.to_owned());
            } else {
                failure_count += 1;
                error.get_or_insert_with(|| message.to_owned());
            }
            issue = Some(classified.worst(issue));
        }
        if let Some(verified_ms) = state.verified_ms {
            oldest_verified = Some(match oldest_verified {
                Some(oldest) => oldest.min(verified_ms),
                None => verified_ms,
            });
        } else {
            all_verified = false;
        }
        match state.parent_id.as_deref() {
            Some(parent_id) => current = parent_id,
            None => break,
        }
    }
    let verified_ms = if seen && all_verified {
        oldest_verified
    } else {
        None
    };
    BranchVerification {
        // 优先给出「确认有问题」的那条：被挡住的文案引用的正是它的原因，用户据此
        // 知道先修哪个节点；只有全链都无法检查时才退回到被挡住的文案。
        error: error.or(blocked_error),
        failure_count,
        unverifiable_count,
        issue,
        verified_ms,
    }
}

/// 按作品粗粒度失效校验状态：链文件被改写后，这些节点必须重新物化校验。
pub(crate) fn clear_artwork_verification(
    transaction: &rusqlite::Transaction<'_>,
    artwork_id: &str,
) -> Result<(), String> {
    transaction
        .execute(
            "UPDATE history_nodes SET verified_ms = NULL, verify_error = NULL
             WHERE artwork_id = ?1",
            [artwork_id],
        )
        .map_err(storage::database_error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::test_support::HistoryFixture;

    /// 批量写入只在达到阈值或 `finish` 时落库；未提交的记录随写入器丢弃。
    #[test]
    fn verification_writer_commits_in_batches_and_discards_the_rest() {
        let fixture = HistoryFixture::new();
        fixture.commit_node(&fixture.main_branch_id, "root", None, 1);

        let mut writer = VerificationWriter::open(&fixture.root).unwrap();
        writer.record_verified("root", 123).unwrap();
        assert_eq!(
            verification_state(&fixture.root),
            (None, None),
            "未 finish 的记录不得落库"
        );
        writer.finish().unwrap();
        assert_eq!(verification_state(&fixture.root), (Some(123), None));

        // 第 128 条触发一次提交；之后累积的一条在丢弃写入器时被放弃。
        let mut writer = VerificationWriter::open(&fixture.root).unwrap();
        for _ in 0..VERIFICATION_BATCH_SIZE {
            writer.record_verified("root", 456).unwrap();
        }
        assert_eq!(
            verification_state(&fixture.root),
            (Some(456), None),
            "达到批量阈值时必须提交"
        );
        writer.record_error("root", "未提交的失败").unwrap();
        drop(writer);
        assert_eq!(
            verification_state(&fixture.root),
            (Some(456), None),
            "取消路径丢弃未提交批次"
        );
    }

    fn verification_state(root: &Path) -> (Option<i64>, Option<String>) {
        storage::open(root)
            .unwrap()
            .query_row(
                "SELECT verified_ms, verify_error FROM history_nodes WHERE id = 'root'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
    }
}
