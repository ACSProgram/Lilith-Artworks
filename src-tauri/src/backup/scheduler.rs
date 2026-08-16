use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::{app::AppState, history, storage};

use super::{worker, BackupState};

const ERROR_RETRY: Duration = Duration::from_secs(60);
const IDLE_RECHECK: Duration = Duration::from_secs(5 * 60);

pub(crate) fn run(state: BackupState, app: AppHandle) {
    loop {
        if !state.wait_scheduler(Duration::ZERO) {
            break;
        }
        let root = match app.state::<AppState>().ready_repository_path() {
            Ok(root) => root,
            _ => {
                if !state.wait_scheduler(ERROR_RETRY) {
                    break;
                }
                continue;
            }
        };
        if app.state::<AppState>().automatic_backups_paused() {
            state.set_automatic_scheduling(false);
            if !state.wait_scheduler(IDLE_RECHECK) {
                break;
            }
            continue;
        }
        state.set_automatic_scheduling(true);
        let branches = match history::list_scheduled(&root) {
            Ok(branches) => branches,
            Err(_) => {
                if !state.wait_scheduler(ERROR_RETRY) {
                    break;
                }
                continue;
            }
        };
        let now = match storage::now_ms() {
            Ok(now) => now,
            Err(_) => {
                if !state.wait_scheduler(ERROR_RETRY) {
                    break;
                }
                continue;
            }
        };
        let mut due = None;
        let mut next_due = None;
        for branch in branches {
            let due_at = branch.retry_at_ms.unwrap_or_else(|| {
                branch.last_check_ms.map_or(now, |checked| {
                    checked
                        .saturating_add(i64::from(branch.interval_minutes) * 60_000)
                        .saturating_add(jitter_ms(&branch.id))
                })
            });
            if due_at <= now {
                due = Some(branch.id);
                break;
            }
            next_due = Some(next_due.map_or(due_at, |current: i64| current.min(due_at)));
        }
        if let Some(branch_id) = due {
            let result = state.run_exclusive(Some(&branch_id), || {
                worker::run_backup(&root, &branch_id, "", "automatic", || state.cancelled())
            });
            if let Err(error) = result {
                let disabled = storage::now_ms()
                    .and_then(|failed_ms| {
                        history::mark_automatic_backup_error(&root, &branch_id, &error, failed_ms)
                    })
                    .unwrap_or(false);
                if disabled {
                    log::error!("automatic backup disabled after repeated failures for branch {branch_id}: {error}");
                } else {
                    log::error!("automatic backup failed for branch {branch_id}: {error}");
                }
                if !state.wait_scheduler(ERROR_RETRY) {
                    break;
                }
            }
            continue;
        }
        let timeout = next_due
            .map(|due_at| Duration::from_millis(due_at.saturating_sub(now) as u64))
            .unwrap_or(IDLE_RECHECK)
            .min(IDLE_RECHECK);
        if !state.wait_scheduler(timeout) {
            break;
        }
    }
}

fn jitter_ms(branch_id: &str) -> i64 {
    let hash = branch_id.bytes().fold(0_u64, |value, byte| {
        value.wrapping_mul(131).wrapping_add(u64::from(byte))
    });
    ((hash % 21) as i64 - 10) * 1_000
}
