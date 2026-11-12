//! 命令面一致性：`src/lib/cmd.ts` 的 `CmdReturn` 必须与 `generate_handler!` 一一对应。
//!
//! 两侧名单分开维护（前端要入参/返回类型，后端要注册 handler），漏改任何一侧
//! 都只会在运行时报 “command not found”。ts-rs 只保证**类型**同步，不保证**名单**同步，
//! 这条测试把名单也钉死，作为 D8 契约的补充。

const CMD_TS: &str = include_str!("../../src/lib/cmd.ts");
const HANDLERS_RS: &str = include_str!("../src/desktop/mod.rs");

/// 取 TypeScript 接口体的键名。只支持平铺成员（`name: T;`），
/// 命令面本身就该是平铺的一列，多行对象类型不进这里。
fn interface_keys(source: &str, header: &str) -> Vec<String> {
    let start = source.find(header).expect("找不到命令接口声明") + header.len();
    let body = &source[start..];
    let end = body.find("\n}").expect("命令接口缺少结束标记");
    body[..end]
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('/') {
                return None;
            }
            let (name, _) = line.split_once(':')?;
            let name = name.trim();
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return None;
            }
            Some(name.to_string())
        })
        .collect()
}

/// 取 `generate_handler!` 里的函数名（`commands::xxx::name` 的末段）。
fn handler_names(source: &str) -> Vec<String> {
    const HEADER: &str = "generate_handler![";
    let start = source.find(HEADER).expect("找不到 generate_handler!") + HEADER.len();
    let body = &source[start..];
    let end = body.find(']').expect("generate_handler! 未闭合");
    body[..end]
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('/') {
                return None;
            }
            line.rsplit("::").next().map(|name| {
                name.trim()
                    .trim_end_matches(',')
                    .trim_end_matches(']')
                    .to_string()
            })
        })
        .filter(|name| !name.is_empty())
        .collect()
}

#[test]
fn command_surface_matches_registered_handlers() {
    let mut ts = interface_keys(CMD_TS, "export interface CmdReturn {");
    let mut rs = handler_names(HANDLERS_RS);
    ts.sort();
    ts.dedup();
    rs.sort();
    rs.dedup();

    assert_eq!(
        ts, rs,
        "src/lib/cmd.ts 与 generate_handler! 的命令名单不一致：先改这张表，再注册 handler"
    );
    assert_eq!(
        ts.len(),
        99,
        "命令面数量变化：确认调用链已全部接好，再更新这里的基线"
    );
}
