use std::path::Path;

use crate::{
    app::command_runtime::{CommandContext, CommandError},
    cleanup,
    library::{
        self, CreateArtworkRequest, LibrarySearchResult, LibraryTrashEntry, LibraryTree,
        MoveLibraryNodesRequest, RepositoryStatus,
    },
};

pub(crate) fn repository_status(
    context: &CommandContext,
) -> Result<RepositoryStatus, CommandError> {
    let app_state = context.app_state();
    let Some(root) = context.repository_path()? else {
        return Ok(RepositoryStatus {
            configured: false,
            ready: false,
            root_path: String::new(),
            database_path: String::new(),
            error: None,
        });
    };
    let database = library::database_path(&root);
    match app_state.ready_repository_path() {
        Ok(ready_root) => Ok(RepositoryStatus {
            configured: true,
            ready: true,
            root_path: ready_root.to_string_lossy().into_owned(),
            database_path: database.to_string_lossy().into_owned(),
            error: None,
        }),
        Err(error) => Ok(RepositoryStatus {
            configured: true,
            ready: false,
            root_path: root.to_string_lossy().into_owned(),
            database_path: database.to_string_lossy().into_owned(),
            error: Some(error),
        }),
    }
}

pub(crate) fn list_tree(context: &CommandContext) -> Result<LibraryTree, CommandError> {
    context.run_read("list_library_tree", |session| {
        library::list_tree(session.root()).map_err(CommandError::from)
    })
}

pub(crate) fn search(
    context: &CommandContext,
    query: String,
) -> Result<Vec<LibrarySearchResult>, CommandError> {
    context.run_read("search_library", |session| {
        library::search(session.root(), &query).map_err(CommandError::from)
    })
}

pub(crate) fn list_trash(context: &CommandContext) -> Result<Vec<LibraryTrashEntry>, CommandError> {
    context.run_read("list_library_trash", |session| {
        library::list_trash(session.root()).map_err(CommandError::from)
    })
}

pub(crate) fn create_group(
    context: &CommandContext,
    parent_id: Option<String>,
    title: String,
) -> Result<LibraryTree, CommandError> {
    context.run_write(|session| {
        library::create_group(session.root(), parent_id.as_deref(), &title)
            .map_err(CommandError::from)
    })
}

pub(crate) fn create_artwork(
    context: &CommandContext,
    request: CreateArtworkRequest,
) -> Result<LibraryTree, CommandError> {
    let quick_default = context.app_state().automatic_backup_quick_default();
    let tree = context.run_write(|session| {
        library::create_artwork_and_list(
            session.root(),
            request.parent_id.as_deref(),
            &request.title,
            &request.branch_title,
            Path::new(&request.source_path),
            quick_default,
        )
        .map_err(CommandError::from)
    })?;
    context.backup_state().wake_scheduler();
    Ok(tree)
}

/// 无头入口需要新建标识而不是完整作品树；与 GUI 共用同一领域实现。
#[cfg(feature = "headless")]
pub(crate) fn create_artwork_created(
    context: &CommandContext,
    parent_id: Option<&str>,
    title: &str,
    branch_title: &str,
    source_path: &Path,
) -> Result<library::CreatedArtwork, CommandError> {
    let quick_default = context.app_state().automatic_backup_quick_default();
    context.run_write(|session| {
        library::create_artwork(
            session.root(),
            parent_id,
            title,
            branch_title,
            source_path,
            quick_default,
        )
        .map_err(CommandError::from)
    })
}

pub(crate) fn rename_node(
    context: &CommandContext,
    id: String,
    title: String,
) -> Result<LibraryTree, CommandError> {
    context.run_write(|session| {
        library::rename_node(session.root(), &id, &title).map_err(CommandError::from)
    })
}

pub(crate) fn trash_nodes(
    context: &CommandContext,
    ids: Vec<String>,
) -> Result<LibraryTree, CommandError> {
    context
        .run_write(|session| library::trash_nodes(session.root(), &ids).map_err(CommandError::from))
}

pub(crate) fn restore_trash(
    context: &CommandContext,
    id: String,
) -> Result<LibraryTree, CommandError> {
    context.run_write(|session| {
        library::restore_trash(session.root(), &id).map_err(CommandError::from)
    })
}

pub(crate) fn move_nodes(
    context: &CommandContext,
    request: MoveLibraryNodesRequest,
) -> Result<LibraryTree, CommandError> {
    context.run_write(|session| {
        library::move_nodes(session.root(), request).map_err(CommandError::from)
    })
}

pub(crate) fn permanently_delete_trash(
    context: &CommandContext,
    ids: Vec<String>,
) -> Result<cleanup::CleanupReport, CommandError> {
    let report = context.run_write(|session| {
        let cleanup_ids =
            library::permanently_delete_trash(session.root(), &ids).map_err(CommandError::from)?;
        cleanup::run(session.root(), &cleanup_ids).map_err(CommandError::from)
    })?;
    context.backup_state().wake_scheduler();
    super::report_cleanup_queue(context);
    Ok(report)
}

pub(crate) fn empty_trash(
    context: &CommandContext,
) -> Result<cleanup::CleanupReport, CommandError> {
    let report = context.run_write(|session| {
        let cleanup_ids = library::empty_trash(session.root()).map_err(CommandError::from)?;
        cleanup::run(session.root(), &cleanup_ids).map_err(CommandError::from)
    })?;
    context.backup_state().wake_scheduler();
    super::report_cleanup_queue(context);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::{
        app::{
            command_runtime::{
                AppEvent, CancelToken, EventSink, OperationDomain, OperationOutcome,
            },
            settings::AppSettings,
            AppState,
        },
        backup::BackupState,
    };

    use super::*;

    #[derive(Default)]
    struct RecordingSink {
        events: Mutex<Vec<AppEvent>>,
    }

    impl EventSink for RecordingSink {
        fn emit(&self, event: &AppEvent) {
            self.events.lock().unwrap().push(event.clone());
        }
    }

    fn coordinator_for(
        root: &std::path::Path,
    ) -> (crate::app::OperationCoordinator, Arc<RecordingSink>) {
        let mut settings = AppSettings::default();
        settings.repository_path = root.to_string_lossy().into_owned();
        let state = AppState::new(
            settings,
            root.join("settings.json"),
            root.join("logs"),
            None,
        );
        let sink = Arc::new(RecordingSink::default());
        (
            crate::app::OperationCoordinator::new(state, BackupState::default(), sink.clone()),
            sink,
        )
    }

    #[test]
    fn create_artwork_use_case_runs_through_the_coordinator_and_reports_the_change() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        library::initialize(&root).unwrap();
        let source = directory.path().join("source.psd");
        std::fs::write(&source, b"initial").unwrap();

        let (coordinator, sink) = coordinator_for(&root);
        let tree = coordinator
            .execute(OperationDomain::Library, |context| {
                create_artwork(
                    context,
                    CreateArtworkRequest {
                        parent_id: None,
                        title: "作品".into(),
                        branch_title: "主线".into(),
                        source_path: source.to_string_lossy().into_owned(),
                    },
                )
            })
            .unwrap();

        assert_eq!(tree.artwork_count, 1);
        assert_eq!(tree.nodes.len(), 1);
        assert_eq!(tree.nodes[0].title, "作品");
        let listed = coordinator
            .execute(OperationDomain::Library, |context| list_tree(context))
            .unwrap();
        assert_eq!(listed.nodes[0].title, "作品");
        let direct = library::list_tree(&root).unwrap();
        assert_eq!(direct.nodes[0].title, "作品");

        let events = sink.events.lock().unwrap();
        assert!(events.iter().any(|event| matches!(
            event.kind,
            crate::app::command_runtime::AppEventKind::RepositoryChanged
        )));
        assert!(events.iter().any(|event| matches!(
            event.kind,
            crate::app::command_runtime::AppEventKind::OperationFinished {
                outcome: OperationOutcome::Succeeded,
                ..
            }
        )));
    }

    /// GUI（默认取消来源）与 headless（显式令牌）必须走同一 use-case，并在相同输入下
    /// 返回相同的错误码、分类与文案。阶段 1 的契约测试以此钉住两入口的共享路径。
    #[test]
    fn gui_and_headless_inputs_share_the_same_error_contract() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        library::initialize(&root).unwrap();
        let source = directory.path().join("source.psd");
        std::fs::write(&source, b"initial").unwrap();
        let (coordinator, _sink) = coordinator_for(&root);
        let request = || CreateArtworkRequest {
            parent_id: None,
            title: String::new(),
            branch_title: "主线".into(),
            source_path: source.to_string_lossy().into_owned(),
        };

        let gui = coordinator
            .execute(OperationDomain::Library, |context| {
                create_artwork(context, request())
            })
            .unwrap_err();
        let headless = coordinator
            .execute_with(
                OperationDomain::Library,
                CancelToken::standalone(),
                |context| create_artwork(context, request()),
            )
            .unwrap_err();

        assert_eq!(gui.code, headless.code);
        assert_eq!(gui.kind, headless.kind);
        assert_eq!(gui.message, headless.message);
        assert_eq!(gui.retryable, headless.retryable);
    }
}
