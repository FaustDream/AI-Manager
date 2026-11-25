//! 启动器的死线：旧后端不能继续服务新前端，仓库布局不能拿旧 exe 冒充新代码，
//! cargo 回退也不能被批处理提前展开吃掉。这里沿用命令名单的文本级契约测试风格——
//! start.cmd 是批处理，无法在单测里真起进程，但这些顺序可以稳定复现。

use std::fs;

fn start_script() -> String {
    fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../start.cmd"))
        .expect("仓库根目录应有 start.cmd")
}

#[test]
fn start_script_stops_stale_backend_before_launch() {
    let script = start_script();
    let kill = "taskkill /F /IM \"ai-manager.exe\"";
    let launch = "start \"AI Manager backend\"";

    let kill_pos = script
        .find(kill)
        .unwrap_or_else(|| panic!("start.cmd 必须在启动前停掉旧后端：{kill}"));
    let launch_pos = script
        .find(launch)
        .unwrap_or_else(|| panic!("start.cmd 应有后端启动命令：{launch}"));
    assert!(
        kill_pos < launch_pos,
        "停旧进程必须发生在启动新后端之前，否则 5274 仍由旧分发表服务"
    );
}

#[test]
fn start_script_rebuilds_repo_backend_before_missing_exe_guard() {
    let script = start_script();
    let build = "\"%CARGO%\" build --bin ai-manager";
    let guard = "if not exist \"%EXE%\" (";

    let build_pos = script
        .find(build)
        .unwrap_or_else(|| panic!("start.cmd 应构建仓库后端：{build}"));
    let guard_pos = script
        .find(guard)
        .unwrap_or_else(|| panic!("start.cmd 应保留构建后的产物检查：{guard}"));
    assert!(
        build_pos < guard_pos,
        "后端构建不能藏在「exe 不存在才构建」的分支里，否则旧 exe 会一直挡住新代码"
    );
}

#[test]
fn cargo_fallback_is_set_before_repo_build_block() {
    let script = start_script();
    let fallback = "if not exist \"%CARGO%\" set \"CARGO=cargo\"";
    let block = "if exist \"%ROOT%src-tauri\\Cargo.toml\" (";

    let fallback_pos = script
        .find(fallback)
        .unwrap_or_else(|| panic!("start.cmd 应有 cargo 回退：{fallback}"));
    let block_pos = script
        .find(block)
        .unwrap_or_else(|| panic!("start.cmd 应有仓库布局判断：{block}"));
    assert!(
        fallback_pos < block_pos,
        "cargo 回退必须写在仓库布局的括号块外：块内赋值会被批处理提前展开，等号右边还是旧路径"
    );
}
