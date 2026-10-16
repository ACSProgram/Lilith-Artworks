//! K 组：输入域极值。
//!
//! F1 覆盖的是「数值 / 长度边界」（备注 500、标题 160、查询 160……），本组补的是
//! **字符域**与**路径域**的极值：非 ASCII、emoji、ZWJ 序列、非 BMP 码位、RTL、
//! 组合附加符号、零宽与不可见字符、内部换行与制表符，以及「标题里带路径分隔符」。
//!
//! 为什么值得专门测：这是 Windows 桌面应用最经典的一类崩溃与数据错乱来源——
//! 长度按字节而不是按字符计数、把用户可见的标题直接当文件名用、非 ASCII 路径在
//! 命令行参数与文件系统之间来回转换时被截断或替换成 ``。
//!
//! 断言仍然只用子命令 JSON 与磁盘事实：标题必须**逐字符往返**（写进去什么，
//! `list-tree` 读回来什么），拒绝必须带说明，之后仓库必须仍能整体校验。
//!
//! 运行：`cargo test --features headless --test stress_input`

// 只在启用 `headless` feature 时编译。理由见 `tests/stress_support/mod.rs` 顶部说明。
#![cfg(feature = "headless")]

mod stress_support;

use std::fs;
use std::path::Path;

use serde_json::json;
use stress_support::*;

/// 用给定标题新建一个 Artwork；工作文件由调用方准备。
fn create_artwork(fixture: &ArtworkFixture, title: &str, source: &Path) -> Outcome {
    fixture
        .command("create-artwork")
        .arg("title", title)
        .arg("branch-title", "Main")
        .arg("source", source.to_string_lossy())
        .finish()
}

/// `list-tree` 的全部节点标题（先序）。
fn titles(fixture: &ArtworkFixture) -> Vec<String> {
    let outcome = fixture.command("list-tree").finish();
    outcome.expect_ok();
    tree_node_titles(&outcome.result["data"])
}

/// K1 字符域：非 ASCII、emoji、ZWJ 序列、非 BMP、RTL、组合附加符号、零宽字符、
/// 内部换行与制表符，以及标题里带路径分隔符。
///
/// 两条断言：① 每一个标题都必须逐字符往返；② 标题不得被当成文件名使用——
/// `slash/back\slash` 与 `colon:star*question?` 若被用于路径，在 Windows 上会直接
/// 失败或创建出意外的目录，因此它们的成功本身就是「标识符与显示名分离」的证据。
/// 末尾用 emoji 标题的作品走一遍提交 / 恢复，确认端到端可用。
#[test]
fn k1_unicode_titles_round_trip_character_for_character() {
    const TITLES: [&str; 15] = [
        "Artwork 中文标题",
        "🎨 palette",
        "👨👩👧👦 family",
        "𝐀𝐁𝐂 𝐌𝐚𝐭𝐡",
        "مرحبا بالعالم",
        "e\u{0301} combining",
        "zero\u{200B}width",
        "日本語のタイトル",
        "\u{FEFF}bom-ish",
        "line\nbreak",
        "tab\tseparated",
        "𝔘𝔫𝔦𝔠𝔬𝔡𝔢 𝔉𝔯𝔞𝔨𝔱𝔲𝔯",
        "slash/back\\slash",
        "colon:star*question?",
        "trailing space ",
    ];

    let fixture = ArtworkFixture::new();
    let mut branches = Vec::with_capacity(TITLES.len());
    let mut first = None;
    for (index, title) in TITLES.iter().enumerate() {
        let source = fixture.workspace.work(&format!("input-{index}.bin"));
        write_work_file(&source, 32 * 1024, 0x700 + index as u64);
        let created = create_artwork(&fixture, title, &source);
        created.expect_ok();
        let branch = created.data_str("branchId");
        assert!(!branch.is_empty(), "{}", created.describe());
        branches.push(branch);
        first.get_or_insert(created);
    }

    let found = titles(&fixture);
    for title in TITLES.iter() {
        // 产品在写入前做 `trim()`，因此往返的期望值是裁掉首尾空白后的标题。
        let expected = title.trim();
        assert!(
            found.iter().any(|value| value == expected),
            "标题没有逐字符往返：期望 {expected:?}，实际 {found:?}"
        );
    }

    // 非 ASCII 子串的搜索必须命中：查询走的是同一条字符处理路径。
    let searched = fixture.command("search").arg("query", "中文").finish();
    searched.expect_ok();
    assert!(
        searched.data_u64("count") >= 1,
        "非 ASCII 查询必须命中中文标题：\n{}",
        searched.describe()
    );

    assert_eq!(fixture.healthy(), 0, "尚未提交，历史应为空");
    fixture.assert_repository_clean();

    // 功能闭环：给 emoji 标题的作品提交两个版本，恢复旧版本必须逐位一致。
    let source = fixture.workspace.work("input-1.bin");
    write_work_file(&source, 40 * 1024, 0x711);
    let committed = fixture
        .command("commit")
        .arg("branch", &branches[1])
        .arg("commit-kind", "manual")
        .arg("note", "emoji v1")
        .finish();
    committed.expect_ok();
    let node = committed.data_str("historyId");
    assert!(!node.is_empty(), "{}", committed.describe());

    write_work_file(&source, 48 * 1024, 0x721);
    fixture
        .command("commit")
        .arg("branch", &branches[1])
        .arg("commit-kind", "manual")
        .arg("note", "emoji v2")
        .finish()
        .expect_ok();

    let output = fixture.workspace.out("k1-oldest.bin");
    fixture
        .command("restore")
        .arg("history", &node)
        .arg("output", output.to_string_lossy())
        .finish()
        .expect_ok();
    assert_eq!(
        fs::read(&output).expect("无法读取恢复输出"),
        pattern_bytes(40 * 1024, 0x711),
        "非 ASCII 标题的作品必须能完整恢复出该版本的字节"
    );
    fs::remove_file(&output).expect("无法清理恢复输出");

    fixture.assert_repository_clean();
    assert_eq!(fixture.healthy(), 2);
    record(
        "K1 unicode titles",
        "K",
        "small",
        &committed,
        json!({ "titles": TITLES.len() }),
    );
}

/// K2 长度按**字符**计数而不是按字节：160 个 4 字节 emoji（640 字节）必须被接受，
/// 161 个必须被拒绝；3 字节 CJK 同理；首尾空白先裁掉再计数。
///
/// 若哪天改成按字节计数，640 字节的标题会被误拒——这条用例会第一时间失败。
#[test]
fn k2_title_length_counts_characters_not_bytes() {
    let fixture = ArtworkFixture::new();
    let source = fixture.workspace.work("k2.bin");
    write_work_file(&source, 8 * 1024, 0x820);

    let emoji = "😀".repeat(160);
    assert_eq!(emoji.len(), 640, "160 个 emoji 应当是 640 字节");
    create_artwork(&fixture, &emoji, &source).expect_ok();

    let cjk = "漢".repeat(160);
    assert_eq!(cjk.len(), 480, "160 个 CJK 应当是 480 字节");
    create_artwork(&fixture, &cjk, &source).expect_ok();

    // 首尾空白不计入长度：`trim` 之后仍是 160 个字符。
    create_artwork(&fixture, &format!("   {}   ", "😀".repeat(160)), &source).expect_ok();

    let found = titles(&fixture);
    assert!(
        found.iter().any(|title| title == &emoji),
        "640 字节 / 160 字符的标题必须被接受并逐字符往返：{found:?}"
    );
    assert!(
        found.iter().any(|title| title == &cjk),
        "480 字节 / 160 字符的标题必须被接受并逐字符往返：{found:?}"
    );

    for over in ["😀".repeat(161), "漢".repeat(161)] {
        let rejected = create_artwork(&fixture, &over, &source);
        assert!(
            !rejected.ok() && !rejected.timed_out,
            "161 个字符的标题必须被拒绝：\n{}",
            rejected.describe()
        );
        assert!(
            rejected.error().contains("160"),
            "拒绝理由应指出 160 的上限：\n{}",
            rejected.describe()
        );
    }

    fixture.assert_repository_clean();
    assert_eq!(fixture.healthy(), 0);
    record(
        "K2 title length by characters",
        "K",
        "small",
        &create_artwork(&fixture, &"t".repeat(161), &source),
        json!({ "accepted": ["emoji160", "cjk160", "padded-emoji160"], "rejected": ["emoji161", "cjk161"] }),
    );
}

/// K3 空白与不可见字符：`trim` 按 Unicode `White_Space` 判定，所以「只由空白组成」
/// 的标题必须被拒绝；而**不属于** `White_Space` 的零宽字符不会被裁掉。
///
/// 后半句是对当前行为的特性记录：`\u{200B}`（零宽空格）、`\u{200D}`（零宽连接符）、
/// `\u{FEFF}`（字节序标记）都能组成一个「看得见是空、实际非空」的标题。这不是崩溃，
/// 但属于一处产品可改进点，已记入 `docs/user/stress-test-report.md` 的边界清单；
/// 若哪天收紧判定，这条用例会先失败，提示把它改成拒绝。
#[test]
fn k3_whitespace_and_invisible_titles_are_handled_explicitly() {
    let fixture = ArtworkFixture::new();
    let source = fixture.workspace.work("k3.bin");
    write_work_file(&source, 8 * 1024, 0x830);

    for blank in [
        "   ", "\t\r\n", "\u{00A0}", "\u{2028}", "\u{2029}", "\u{0085}",
    ] {
        let rejected = create_artwork(&fixture, blank, &source);
        assert!(
            !rejected.ok() && !rejected.timed_out,
            "只由空白（{blank:?}）组成的标题必须被拒绝：\n{}",
            rejected.describe()
        );
        assert!(
            !rejected.error().is_empty(),
            "拒绝必须带说明：\n{}",
            rejected.describe()
        );
    }

    for invisible in ["\u{200B}", "\u{200D}", "\u{FEFF}"] {
        let created = create_artwork(&fixture, invisible, &source);
        created.expect_ok();
        assert!(
            titles(&fixture).iter().any(|title| title == invisible),
            "非空白的不可见字符必须逐字符往返：{invisible:?}"
        );
    }

    fixture.assert_repository_clean();
    assert_eq!(fixture.healthy(), 0);
    record(
        "K3 whitespace and invisible titles",
        "K",
        "small",
        &create_artwork(&fixture, "   ", &source),
        json!({ "rejected": ["spaces", "tab-crlf", "nbsp", "line-sep", "para-sep", "nel"], "accepted": ["zwsp", "zwj", "bom"] }),
    );
}

/// K4 路径域：非 ASCII 目录与文件名、路径中的空格、以及较深的嵌套。
///
/// 命令行参数在 Rust 子进程里走 UTF-16、文件系统走 NTFS，两者之间来回转换时出现
/// 替换字符或截断是 Windows 上的经典故障，所以这里断言的是**端到端可用**：
/// 建库 → 两次提交 → 恢复，且恢复产物逐位一致。
#[test]
fn k4_non_ascii_and_spaced_paths_work_end_to_end() {
    let fixture = ArtworkFixture::new();
    let mut directory = fixture.workspace.work_directory();
    for segment in ["含空格 与中文 的目录", "nested", "deeper", "最深层 🎨"] {
        directory = directory.join(segment);
    }
    fs::create_dir_all(&directory).expect("无法创建非 ASCII 深层目录");
    let source = directory.join("作品 草案 🎨 v2.bin");
    write_work_file(&source, 24 * 1024, 0x940);

    let created = create_artwork(&fixture, "Path domain", &source);
    created.expect_ok();
    let branch = created.data_str("branchId");

    write_work_file(&source, 28 * 1024, 0x950);
    let committed = fixture
        .command("commit")
        .arg("branch", &branch)
        .arg("commit-kind", "manual")
        .arg("note", "path v1")
        .finish();
    committed.expect_ok();
    let node = committed.data_str("historyId");

    write_work_file(&source, 30 * 1024, 0x960);
    fixture
        .command("commit")
        .arg("branch", &branch)
        .arg("commit-kind", "manual")
        .arg("note", "path v2")
        .finish()
        .expect_ok();

    let output = fixture.workspace.out("k4-v1.bin");
    fixture
        .command("restore")
        .arg("history", &node)
        .arg("output", output.to_string_lossy())
        .finish()
        .expect_ok();
    assert_eq!(
        fs::read(&output).expect("无法读取恢复输出"),
        pattern_bytes(28 * 1024, 0x950),
        "非 ASCII 路径的作品必须能完整恢复出该版本的字节"
    );
    fs::remove_file(&output).expect("无法清理恢复输出");

    fixture.assert_repository_clean();
    assert_eq!(fixture.healthy(), 2);
    record(
        "K4 non-ascii paths",
        "K",
        "small",
        &committed,
        json!({ "pathChars": source.to_string_lossy().chars().count() }),
    );
}

/// K5 路径域的另一端：相对路径、工作区之外的路径、以及工作区内不存在的文件
/// 都必须被**明确拒绝**，且不得留下副作用。
///
/// 无头入口把 `--source` 约束在 `--workspace` 之内（与「待识别图片」共用同一套
/// 授权），这条边界此前没有测试。
#[test]
fn k5_relative_and_out_of_workspace_sources_are_rejected() {
    let fixture = ArtworkFixture::new();

    let relative = create_artwork(&fixture, "Relative", Path::new("relative.bin"));
    assert!(
        !relative.ok() && !relative.timed_out,
        "相对路径必须被拒绝：\n{}",
        relative.describe()
    );
    assert!(
        relative.error().contains("绝对路径"),
        "拒绝理由应指明必须使用绝对路径：\n{}",
        relative.describe()
    );

    let outside = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    assert!(outside.is_file(), "夹具路径必须存在：{}", outside.display());
    let escaped = create_artwork(&fixture, "Outside", &outside);
    assert!(
        !escaped.ok() && !escaped.timed_out,
        "工作区之外的路径必须被拒绝：\n{}",
        escaped.describe()
    );
    assert!(
        escaped.error().contains("--workspace"),
        "拒绝理由应指明工作区边界：\n{}",
        escaped.describe()
    );

    // 工作区内但不存在：必须如实报「不存在」，且不得凭空创建源文件。
    let missing = fixture.workspace.work("does-not-exist.bin");
    let absent = create_artwork(&fixture, "Missing", &missing);
    assert!(
        !absent.ok() && !absent.timed_out,
        "不存在的源文件必须被拒绝：\n{}",
        absent.describe()
    );
    assert!(
        absent.error().contains("不存在"),
        "拒绝理由应如实说明文件不存在：\n{}",
        absent.describe()
    );
    assert!(!missing.exists(), "被拒绝的创建不得凭空创建源文件");

    fixture.assert_repository_clean();
    assert_eq!(fixture.healthy(), 0);
    record(
        "K5 path boundaries",
        "K",
        "small",
        &escaped,
        json!({ "rejected": ["relative", "outside-workspace", "missing-in-workspace"] }),
    );
}
