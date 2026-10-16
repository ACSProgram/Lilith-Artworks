//! 门控守护：`tests/stress_*.rs` 必须都带 `#![cfg(feature = "headless")]`。
//!
//! Cargo 会自动发现 `tests/` 下的**每一个** `.rs` 文件并运行，因此一个压力测试目标放在
//! 这里就等于「默认参与裸 `cargo test`」。`#![cfg(feature = "headless")]` 是唯一的门控：
//! 不带该 feature 时目标被编译为空，既不构建无头二进制、也不启动任何进程。
//!
//! 这一行若漏写，裸 `cargo test` 就会去启动应用——`--headless` 被忽略，debug 构建加载
//! devUrl，在 CI 或本地弹出一个「找不到网页」的 GUI 窗口并挂住。Cargo 不会替我们检查
//! 这件事（文件放在 `tests/` 就等于默认参与），所以用这个守护测试把它钉死。
//!
//! 本文件**自身**不带 `headless` 门控：它必须在裸 `cargo test` 下运行，这正是它要守护的
//! 那个场景。它只读 `tests/` 目录与文件文本，不启动任何进程。
//!
//! 运行：`cargo test --test gate_guard`（也随裸 `cargo test` 一起跑）。

use std::{fs, path::PathBuf};

/// 压力测试目标必须逐字包含的门控行。
const REQUIRED_GATE: &str = "#![cfg(feature = \"headless\")]";

fn tests_directory() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests")
}

/// 收集 `tests/` 顶层所有 `stress_*.rs` 目标（不含子目录，也不含本文件）。
fn stress_targets() -> Vec<(String, PathBuf)> {
    let mut targets = Vec::new();
    let entries = fs::read_dir(tests_directory()).expect("无法读取 tests 目录");
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("stress_") && name.ends_with(".rs") {
            targets.push((name, path));
        }
    }
    targets.sort();
    targets
}

/// 每个 `stress_*.rs` 都必须带门控行；漏写时给出可行动的失败信息。
#[test]
fn every_stress_target_is_gated_behind_the_headless_feature() {
    let targets = stress_targets();
    assert!(
        !targets.is_empty(),
        "未在 {} 找到任何 stress_*.rs 目标——守护测试自身可能失效",
        tests_directory().display()
    );

    let mut missing = Vec::new();
    for (name, path) in &targets {
        let source = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("无法读取 {}：{error}", path.display()));
        if !source.lines().any(|line| line.trim() == REQUIRED_GATE) {
            missing.push(name.clone());
        }
    }

    assert!(
        missing.is_empty(),
        "以下压力测试目标缺少门控行 `{REQUIRED_GATE}`：{missing:?}\n\
         漏写会让裸 `cargo test` 去启动 GUI（`--headless` 被忽略，弹出「找不到网页」窗口）。\
         在文件开头补上这一行即可。"
    );
}

/// 反向自检：守护测试必须能识别一个**确实缺少**门控的样本，否则它可能永远通过。
///
/// 直接对一个临时构造的文本做判定，不写盘、不影响任何目标文件。
#[test]
fn the_gate_check_would_actually_catch_a_missing_gate() {
    let without_gate = "//! 一个忘记写门控的目标\nmod stress_support;\n\n#[test]\nfn x() {}\n";
    let with_gate = format!("//! 注释\n{REQUIRED_GATE}\n\n#[test]\nfn x() {{}}\n");
    let has_gate = |source: &str| source.lines().any(|line| line.trim() == REQUIRED_GATE);
    assert!(!has_gate(without_gate), "缺少门控的样本必须被判为不合规");
    assert!(has_gate(&with_gate), "带门控的样本必须被判为合规");
}
