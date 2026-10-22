//! 设置文件模型：字段、默认值、迁移、加载修正与保存校验。
//!
//! 这里只描述 `settings.json` 的形状与合法性；写盘经 `write_json_atomic`，写入顺序与
//! revision/CAS 由 `service` 负责。不包含窗口恢复、命令或仓库租约。

use std::{fs, io::Write, path::Path};

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

/// 主窗口允许的最小物理尺寸，与 `tauri.conf.json` 的 `minWidth` / `minHeight` 一致。
/// 加载设置时用它夹紧越界值，捕获窗口几何时用它判断一次捕获是否可信。
pub(super) const MIN_WINDOW_WIDTH: u32 = 760;
pub(super) const MIN_WINDOW_HEIGHT: u32 = 560;
/// 主窗口允许的最大物理尺寸；超过它几乎只可能来自异常状态或异常的显示器几何。
pub(super) const MAX_WINDOW_WIDTH: u32 = 16_384;
pub(super) const MAX_WINDOW_HEIGHT: u32 = 16_384;

pub(crate) const CURRENT_SETTINGS_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct AppSettings {
    pub(crate) version: u32,
    /// 设置文件自己的单调计数：每次成功写入 +1。设置页保存用它做 CAS；它与
    /// schema、ChunkFile 格式和发布版本号无关。
    #[serde(default)]
    pub(crate) revision: u64,
    pub(crate) repository_path: String,
    pub(super) theme: String,
    pub(super) close_to_tray: bool,
    pub(crate) pause_automatic_backups: bool,
    /// 空闲完整性检查开关。默认关闭；旧设置文件缺少该字段时同样视为关闭，
    /// 因此不需要提升设置版本。
    pub(crate) idle_integrity_check_enabled: bool,
    pub(super) automatic_backup_check_mode: String,
    /// ChunkFile v2 写入开关。默认开启：只影响此后新写出的 snapshot/delta，
    /// 旧 v1 节点继续由 v1 reader 读取；关闭后新写入回到 v1。
    pub(super) chunk_file_format_v2: bool,
    pub(super) window: WindowSettings,
    pub(super) content: ContentSettings,
    pub(super) pin_board: PinBoardSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            version: CURRENT_SETTINGS_VERSION,
            revision: 0,
            repository_path: String::new(),
            theme: "system".into(),
            close_to_tray: true,
            pause_automatic_backups: false,
            idle_integrity_check_enabled: false,
            automatic_backup_check_mode: "quick".into(),
            chunk_file_format_v2: true,
            window: WindowSettings::default(),
            content: ContentSettings::default(),
            pin_board: PinBoardSettings::default(),
        }
    }
}

/// 素材板显示与性能设置。缓存等级与间距由前端模块和原生纹理缓存预算共同消费。
/// 新增字段依赖容器级 `#[serde(default)]` 兼容旧设置文件，无需提升设置版本。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct PinBoardSettings {
    pub(super) texture_cache_level: String,
    /// 阵列间距：相对统一图片高度的比例，`0.0` 表示图片紧贴。整理与导入共用。
    /// 旧字段 `arrangementGapPx` 已被容器级 `#[serde(default)]` 静默忽略，不需要
    /// 提升设置版本。
    pub(super) arrangement_gap_ratio: f64,
    /// 编辑停顿后的防抖自动保存（前端行为），默认关闭。
    pub(super) autosave: bool,
    /// 退出应用前结算并保存素材板（关闭握手），默认开启。
    pub(super) save_on_exit: bool,
    pub(super) lock_shortcut: String,
    pub(super) fullscreen_shortcut: String,
}

impl Default for PinBoardSettings {
    fn default() -> Self {
        Self {
            texture_cache_level: "medium".into(),
            arrangement_gap_ratio: crate::pin_board::arrange::DEFAULT_GAP_RATIO,
            autosave: false,
            save_on_exit: true,
            lock_shortcut: "CommandOrControl+R".into(),
            fullscreen_shortcut: "F11".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct WindowSettings {
    pub(super) x: Option<i32>,
    pub(super) y: Option<i32>,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) maximized: bool,
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self {
            x: None,
            y: None,
            width: 1320,
            height: 840,
            maximized: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct ContentSettings {
    pub(super) density: String,
}

impl Default for ContentSettings {
    fn default() -> Self {
        Self {
            density: "comfortable".into(),
        }
    }
}

/// 旧版本设置迁移。
///
/// v1 曾把素材板“锁定画板”的默认键设为 `CommandOrControl+Shift+K`；该默认值已改回
/// Client 的 `CommandOrControl+R`（整页刷新由应用层拦截），因此把仍是旧默认值的
/// 持久化设置一并升级，用户自定义过的其他键位保持不变。
fn migrate_settings(settings: &mut AppSettings) {
    if settings.version < 2 && settings.pin_board.lock_shortcut == "CommandOrControl+Shift+K" {
        settings.pin_board.lock_shortcut = "CommandOrControl+R".into();
    }
}

/// 快捷键允许留空（表示未设置）；非空时按键组合必须由非空段组成，长度受限。
fn shortcut_is_well_formed(shortcut: &str) -> bool {
    let shortcut = shortcut.trim();
    shortcut.is_empty()
        || (shortcut.chars().count() <= 64
            && !shortcut.split('+').any(|part| part.trim().is_empty()))
}

/// 加载落盘设置时的宽松修正：只把越界的个别字段改回可用值，绝不整份丢弃。
///
/// 与 [`validate_settings`] 的分工不同——后者服务于保存入口，用户输入必须明确拒绝；
/// 加载时一个坏字段（典型来源是窗口最小化退出时被写进来的任务栏图标尺寸）不应该把
/// 用户的仓库路径和全部偏好一起清空。返回是否做过修正，供调用点决定是否留下告警日志。
fn sanitize_settings(settings: &mut AppSettings) -> bool {
    let fallback = AppSettings::default();
    let mut repaired = false;

    if !matches!(settings.theme.as_str(), "system" | "light" | "dark") {
        settings.theme = fallback.theme;
        repaired = true;
    }
    if !matches!(
        settings.automatic_backup_check_mode.as_str(),
        "quick" | "full"
    ) {
        settings.automatic_backup_check_mode = fallback.automatic_backup_check_mode;
        repaired = true;
    }
    if !matches!(settings.content.density.as_str(), "comfortable" | "compact") {
        settings.content.density = fallback.content.density;
        repaired = true;
    }
    if !matches!(
        settings.pin_board.texture_cache_level.as_str(),
        "low" | "medium" | "high"
    ) {
        settings.pin_board.texture_cache_level = fallback.pin_board.texture_cache_level;
        repaired = true;
    }
    if !(0.0..=crate::pin_board::arrange::MAX_GAP_RATIO)
        .contains(&settings.pin_board.arrangement_gap_ratio)
    {
        settings.pin_board.arrangement_gap_ratio = fallback.pin_board.arrangement_gap_ratio;
        repaired = true;
    }
    if !shortcut_is_well_formed(&settings.pin_board.lock_shortcut) {
        settings.pin_board.lock_shortcut = fallback.pin_board.lock_shortcut.clone();
        repaired = true;
    }
    if !shortcut_is_well_formed(&settings.pin_board.fullscreen_shortcut) {
        settings.pin_board.fullscreen_shortcut = fallback.pin_board.fullscreen_shortcut.clone();
        repaired = true;
    }

    // 窗口尺寸只夹紧到合法范围，而不是判整份设置无效：越界尺寸最常见的来源是
    // 最小化时捕获到的任务栏图标矩形（约 237×39），夹回最小尺寸即可让窗口继续可见。
    let width = settings
        .window
        .width
        .clamp(MIN_WINDOW_WIDTH, MAX_WINDOW_WIDTH);
    if width != settings.window.width {
        settings.window.width = width;
        repaired = true;
    }
    let height = settings
        .window
        .height
        .clamp(MIN_WINDOW_HEIGHT, MAX_WINDOW_HEIGHT);
    if height != settings.window.height {
        settings.window.height = height;
        repaired = true;
    }

    repaired
}

pub(crate) fn load_settings(path: &Path) -> (AppSettings, Option<String>) {
    if !path.exists() {
        let settings = AppSettings::default();
        let warning = write_json_atomic(path, &settings)
            .err()
            .map(|error| format!("无法创建默认设置：{error}"));
        return (settings, warning);
    }

    match fs::read_to_string(path) {
        Ok(content) => match serde_json::from_str::<AppSettings>(&content) {
            Ok(mut settings) if settings.version == CURRENT_SETTINGS_VERSION => {
                // 越界字段就地修正而不是整份回落默认：曾经把仓库路径连同全部偏好
                // 一起清空。修正只发生在内存里，一致性交回既有写入路径（保存设置或
                // 退出时捕获窗口），启动阶段不额外写盘。
                if sanitize_settings(&mut settings) {
                    log::warn!(
                        "settings.json had out-of-range fields; repaired them in place without dropping the rest"
                    );
                }
                (settings, None)
            }
            Ok(settings) if settings.version > CURRENT_SETTINGS_VERSION => (
                settings,
                Some("settings.json 来自更高版本，本次运行不会覆盖它".into()),
            ),
            Ok(mut settings) => {
                migrate_settings(&mut settings);
                settings.version = CURRENT_SETTINGS_VERSION;
                sanitize_settings(&mut settings);
                let warning = write_json_atomic(path, &settings)
                    .err()
                    .map(|error| format!("设置迁移后无法写回：{error}"));
                (settings, warning)
            }
            Err(error) => (
                AppSettings::default(),
                Some(format!("settings.json 格式无效，已临时使用默认值：{error}")),
            ),
        },
        Err(error) => (
            AppSettings::default(),
            Some(format!("无法读取 settings.json，已临时使用默认值：{error}")),
        ),
    }
}

pub(crate) fn validate_settings(settings: &AppSettings) -> Result<(), String> {
    if !matches!(settings.theme.as_str(), "system" | "light" | "dark") {
        return Err("主题设置无效".into());
    }
    if !matches!(
        settings.automatic_backup_check_mode.as_str(),
        "quick" | "full"
    ) {
        return Err("自动备份检查方式设置无效".into());
    }
    if !matches!(settings.content.density.as_str(), "comfortable" | "compact") {
        return Err("内容密度设置无效".into());
    }
    if !matches!(
        settings.pin_board.texture_cache_level.as_str(),
        "low" | "medium" | "high"
    ) {
        return Err("素材板纹理缓存等级无效".into());
    }
    if !(0.0..=crate::pin_board::arrange::MAX_GAP_RATIO)
        .contains(&settings.pin_board.arrangement_gap_ratio)
    {
        return Err("素材板阵列间距超出允许范围".into());
    }
    for (label, shortcut) in [
        ("锁定画板", &settings.pin_board.lock_shortcut),
        ("画板全屏", &settings.pin_board.fullscreen_shortcut),
    ] {
        if !shortcut_is_well_formed(shortcut) {
            return Err(format!("素材板{label}快捷键格式无效"));
        }
    }
    if settings.window.width < MIN_WINDOW_WIDTH
        || settings.window.height < MIN_WINDOW_HEIGHT
        || settings.window.width > MAX_WINDOW_WIDTH
        || settings.window.height > MAX_WINDOW_HEIGHT
    {
        return Err("窗口尺寸超出允许范围".into());
    }
    if !settings.repository_path.trim().is_empty() {
        let path = Path::new(settings.repository_path.trim());
        if !path.is_absolute() {
            return Err("作品仓库必须使用绝对目录路径".into());
        }
        if path.parent().is_none() || path.file_name().is_none() {
            return Err("不能把磁盘或文件系统根目录用作作品仓库".into());
        }
        if path.exists() && !path.is_dir() {
            return Err("作品仓库路径不是目录".into());
        }
    }
    Ok(())
}

pub(super) fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let parent = path.parent().ok_or("设置文件路径无效")?;
    fs::create_dir_all(parent).map_err(|error| format!("无法创建设置目录：{error}"))?;
    let mut temporary =
        NamedTempFile::new_in(parent).map_err(|error| format!("无法创建临时设置：{error}"))?;
    serde_json::to_writer_pretty(&mut temporary, value)
        .map_err(|error| format!("无法序列化设置：{error}"))?;
    temporary
        .write_all(b"\n")
        .map_err(|error| format!("无法写入设置：{error}"))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| format!("无法同步设置：{error}"))?;
    temporary
        .persist(path)
        .map_err(|error| format!("无法替换设置文件：{}", error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::fuzz_support::random_bytes;

    #[test]
    fn default_settings_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let expected = AppSettings::default();
        write_json_atomic(&path, &expected).unwrap();
        let (actual, warning) = load_settings(&path);
        assert!(warning.is_none());
        assert_eq!(actual.version, CURRENT_SETTINGS_VERSION);
        assert_eq!(actual.window.width, 1320);
        assert_eq!(actual.pin_board.lock_shortcut, "CommandOrControl+R");
        assert_eq!(actual.pin_board.fullscreen_shortcut, "F11");
    }

    #[test]
    fn legacy_content_fields_are_ignored_on_load() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        fs::write(
            &path,
            "{\"version\":2,\"content\":{\"density\":\"compact\",\"defaultPanel\":\"history\"}}",
        )
        .unwrap();

        let (settings, warning) = load_settings(&path);
        assert!(warning.is_none());
        assert_eq!(settings.content.density, "compact");
    }

    #[test]
    fn pin_board_shortcut_validation_accepts_defaults_and_rejects_empty_parts() {
        let mut settings = AppSettings::default();
        validate_settings(&settings).unwrap();

        settings.pin_board.lock_shortcut = "CommandOrControl++".into();
        assert!(validate_settings(&settings).is_err());

        settings.pin_board.lock_shortcut = String::new();
        settings.pin_board.fullscreen_shortcut = String::new();
        validate_settings(&settings).unwrap();
    }

    #[test]
    fn migrates_the_v1_pin_board_lock_default() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");

        let mut legacy = AppSettings::default();
        legacy.version = 1;
        legacy.pin_board.lock_shortcut = "CommandOrControl+Shift+K".into();
        write_json_atomic(&path, &legacy).unwrap();

        let (actual, warning) = load_settings(&path);
        assert!(warning.is_none());
        assert_eq!(actual.version, CURRENT_SETTINGS_VERSION);
        // v1 的旧默认键位随迁移升级为 Client 的 Ctrl+R，否则用户升级后 Ctrl+R 仍不生效。
        assert_eq!(actual.pin_board.lock_shortcut, "CommandOrControl+R");
    }

    #[test]
    fn settings_migration_preserves_a_custom_lock_shortcut() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");

        let mut legacy = AppSettings::default();
        legacy.version = 1;
        legacy.pin_board.lock_shortcut = "CommandOrControl+Alt+L".into();
        write_json_atomic(&path, &legacy).unwrap();

        let (actual, warning) = load_settings(&path);
        assert!(warning.is_none());
        assert_eq!(actual.version, CURRENT_SETTINGS_VERSION);
        assert_eq!(actual.pin_board.lock_shortcut, "CommandOrControl+Alt+L");
    }

    #[test]
    fn atomic_settings_write_replaces_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let initial = AppSettings::default();
        write_json_atomic(&path, &initial).unwrap();

        let mut updated = initial;
        updated.theme = "dark".into();
        updated.window.width = 1440;
        write_json_atomic(&path, &updated).unwrap();

        let (actual, warning) = load_settings(&path);
        assert!(warning.is_none());
        assert_eq!(actual.theme, "dark");
        assert_eq!(actual.window.width, 1440);
    }

    #[test]
    fn an_out_of_range_window_size_keeps_the_rest_of_the_settings() {
        // 复现真实故障：窗口在最小化状态下退出时，`GetWindowRect` 返回任务栏图标
        // 矩形（约 237×39）并被写进 settings.json；此前加载会因此把整份设置回落
        // 默认，用户的仓库路径与全部偏好一起丢失。
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let mut broken = AppSettings::default();
        broken.repository_path = "F:\\LilithData\\Artworks".into();
        broken.pin_board.arrangement_gap_ratio = 0.12;
        broken.pin_board.fullscreen_shortcut = "Escape".into();
        broken.window.width = 237;
        broken.window.height = 39;
        write_json_atomic(&path, &broken).unwrap();

        let (loaded, warning) = load_settings(&path);
        assert!(warning.is_none());
        assert_eq!(loaded.repository_path, "F:\\LilithData\\Artworks");
        assert_eq!(loaded.pin_board.arrangement_gap_ratio, 0.12);
        assert_eq!(loaded.pin_board.fullscreen_shortcut, "Escape");
        // 越界尺寸被夹回可用范围，窗口仍然可见、可拖动。
        assert_eq!(loaded.window.width, MIN_WINDOW_WIDTH);
        assert_eq!(loaded.window.height, MIN_WINDOW_HEIGHT);
    }

    #[test]
    fn sanitizing_repairs_only_the_out_of_range_fields() {
        let mut settings = AppSettings::default();
        settings.repository_path = "F:\\LilithData\\Artworks".into();
        settings.theme = "neon".into();
        settings.pin_board.arrangement_gap_ratio = 0.5;
        settings.window.width = 237;

        assert!(sanitize_settings(&mut settings));
        assert_eq!(settings.repository_path, "F:\\LilithData\\Artworks");
        assert_eq!(settings.theme, "system");
        assert_eq!(
            settings.pin_board.arrangement_gap_ratio,
            crate::pin_board::arrange::DEFAULT_GAP_RATIO
        );
        assert_eq!(settings.window.width, MIN_WINDOW_WIDTH);
        // 修正后的设置已经合法，再次加载不再触发修正。
        assert!(!sanitize_settings(&mut settings));
    }

    /// 畸形输入模糊（设置文件）：任意字节都必须让 `load_settings` 干净地回落并**如实给出
    /// 警告**，绝不 panic；两条断言分别锁定「回落结果合法」与「用户文件不被悄悄覆盖」：
    /// ① 版本为当前版本时（`sanitize_settings` 会运行），返回的设置必须通过
    ///    `validate_settings` —— 这是「修正在内存里、不整份清空」的契约；
    /// ② **解析失败的设置文件必须被原样保留**：它可能是更高版本写入的，一次失败的读取
    ///    绝不能把它覆盖成默认值（那才是真正的数据丢失）。
    #[test]
    fn fuzzed_settings_files_fall_back_without_clobbering_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");

        let mut cases: Vec<Vec<u8>> = Vec::new();
        for seed in 0..64_u64 {
            cases.push(random_bytes(seed, (seed as usize * 17) % 512));
        }
        for text in [
            "",
            "{}",
            "[]",
            "null",
            "123",
            "\"text\"",
            "{",
            "{\"version\":}",
            "{\"version\":99}",
            "{\"version\":-1}",
            "{\"version\":4294967296}",
            "{\"theme\":\"bogus\"}",
            "{\"content\":{\"density\":\"huge\"}}",
        ] {
            cases.push(text.as_bytes().to_vec());
        }
        // 两类经典的「解析器耗尽」手法：超深嵌套与超长字面量。二者都必须被干净拒绝，
        // 而不是把栈或内存吃光。
        cases.push(format!("{}{}", "[".repeat(10_000), "]".repeat(10_000)).into_bytes());
        cases.push("9".repeat(100_000).into_bytes());
        cases.push(format!("{{\"theme\":\"{}\"}}", "x".repeat(100_000)).into_bytes());

        for (index, bytes) in cases.iter().enumerate() {
            fs::write(&path, bytes).unwrap();
            let parses = std::str::from_utf8(bytes)
                .ok()
                .is_some_and(|text| serde_json::from_str::<AppSettings>(text).is_ok());

            let (settings, warning) = load_settings(&path);

            if parses && settings.version == CURRENT_SETTINGS_VERSION {
                assert!(
                    validate_settings(&settings).is_ok(),
                    "case #{index}：当前版本的设置必须被就地修正为合法（不整份回落）"
                );
            }
            if !parses {
                assert!(
                    warning.is_some(),
                    "case #{index}：解析失败必须如实给出警告，而不是静默回落"
                );
                assert_eq!(
                    fs::read(&path).unwrap(),
                    *bytes,
                    "case #{index}：解析失败的设置文件必须被原样保留"
                );
            }
        }
    }
}
