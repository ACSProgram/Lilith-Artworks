#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // 无头模式是开发/验证期入口，由 `headless` feature 门控；发布构建里这一段
    // 整体不存在，因此产品行为零变化。它不创建窗口、不建托盘、不加载 webview、不经过 IPC。
    #[cfg(feature = "headless")]
    {
        let mut arguments = std::env::args();
        let _program = arguments.next();
        let rest = arguments.collect::<Vec<String>>();
        if rest.first().map(String::as_str) == Some("--headless") {
            std::process::exit(lilith_artworks_lib::run_headless(&rest[1..]));
        }
    }
    lilith_artworks_lib::run();
}
