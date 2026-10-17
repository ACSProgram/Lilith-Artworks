//! 仓库之外读写路径的授权闸门（Tauri 文件系统 scope）。
//!
//! 「这条路径必须被用户显式选择过」这条语义由 Tauri 的 filesystem scope 承载：
//! 原生文件选择器会把选中的路径写进 scope（`tauri-plugin-dialog`），应用也可以在
//! 运行时自己加入（拖放落盘、剪贴板里的文件路径）。认证与素材板都要读仓库之外的
//! 文件，因此这条检查不放在任何一个领域模块里——放在两边之上、领域之外，避免
//! 领域模块互相导入。
//!
//! 抽象成可注入的 [`PathAuthorization`] 是为了让无头进程复用**同一条**检查，而不是
//! 为了把检查关掉：无头侧的实现是一个只允许 `--workspace` 之下路径的显式作用域
//! （见 `src/headless.rs`）。两条实现都要求路径被显式授权，且都不接受空路径，因此
//! 不存在「安全控制可关闭」的开关。

use std::path::Path;

use tauri_plugin_fs::FsExt;

/// 路径授权来源。
pub(crate) trait PathAuthorization {
    fn is_path_authorized(&self, path: &Path) -> bool;
}

impl PathAuthorization for tauri::fs::Scope {
    fn is_path_authorized(&self, path: &Path) -> bool {
        self.is_allowed(path)
    }
}

/// 要求路径已被显式授权。空路径一律拒绝。
pub(crate) fn ensure_authorized(
    scope: &impl PathAuthorization,
    path: &Path,
    label: &str,
) -> Result<(), String> {
    if path.as_os_str().is_empty() || !scope.is_path_authorized(path) {
        return Err(format!("{label}未由文件选择器授权，请重新选择"));
    }
    Ok(())
}

/// 把一条由应用自己取得的路径加入窗口的文件系统 scope。
///
/// 用于用户显式触发、但路径来源不是文件选择器的入口（剪贴板里的文件路径），使后续
/// 命令仍走同一条「必须已被授权」的检查，而不是另开一条绕过检查的通道。
pub(crate) fn authorize_path(window: &tauri::WebviewWindow, path: &Path) -> Result<(), String> {
    window
        .fs_scope()
        .allow_file(path)
        .map_err(|error| format!("无法授权该路径：{error}"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// 显式授权的作用域替身：只有被列出的路径才算已授权。
    struct StubScope(Vec<PathBuf>);

    impl PathAuthorization for StubScope {
        fn is_path_authorized(&self, path: &Path) -> bool {
            self.0.iter().any(|allowed| allowed == path)
        }
    }

    #[test]
    fn requires_an_authorized_non_empty_path() {
        let authorized = PathBuf::from("C:/authorized/staged.png");
        let scope = StubScope(vec![authorized.clone()]);

        assert!(ensure_authorized(&scope, &authorized, "待识别图片").is_ok());
        assert!(
            ensure_authorized(&scope, Path::new("C:/elsewhere/secret.png"), "待识别图片").is_err()
        );
        // 空路径始终拒绝，不存在「未选择即放行」的分支。
        assert!(ensure_authorized(&scope, Path::new(""), "待识别图片").is_err());
    }
}
