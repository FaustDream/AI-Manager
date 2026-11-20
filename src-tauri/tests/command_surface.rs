//! D8：命令名单的两份手工清单必须与前端调用面对齐。
//! 类型由 ts-rs 生成，但「有哪些命令」这份名单只有一处登记：
//! `desktop/web.rs` 的分发表。
//! 任何一处漏改，编译照样通过，用户点的却是「未知命令」。
//!
//! 这里做文本级比对：新增/删除命令时，若分发表或 cmd.ts 忘了同步，本测试变红。

use std::collections::BTreeSet;

/// 从 web.rs 的分发表抓命令名：形如 `"account_list" => …` 的分支头，
/// 兼容 `"a" | "b" => …` 的多名分支。只取 `dispatch` 函数体——
/// 同文件里 mime_of 的扩展名匹配臂长得一样，不是命令。
fn dispatch_commands() -> BTreeSet<String> {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/desktop/web.rs"))
        .unwrap();
    let body = text
        .split("pub async fn dispatch")
        .nth(1)
        .expect("web.rs 里应有 dispatch 分发表");
    let mut set = BTreeSet::new();
    for line in body.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('"') || !trimmed.contains("=>") {
            continue;
        }
        let head = trimmed.split("=>").next().unwrap_or("");
        let mut rest = head;
        while let Some(start) = rest.find('"') {
            let after = &rest[start + 1..];
            let Some(end) = after.find('"') else { break };
            let name = &after[..end];
            if name.chars().all(|c| c.is_ascii_lowercase() || c == '_') && !name.is_empty() {
                set.insert(name.to_string());
            }
            rest = &after[end + 1..];
        }
    }
    set
}

/// 从 cmd.ts 的 CmdReturn 接口抓键名（行首两空格 + 键 + 冒号）。
/// 接口体到「独占一行的右括号」为止——值里有内联对象类型（`{ name: string }`），
/// 按第一个 `}` 截断会把名单拦腰砍断。
fn frontend_commands() -> BTreeSet<String> {
    let text =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../src/lib/cmd.ts")).unwrap();
    let body = text
        .split("export interface CmdReturn")
        .nth(1)
        .expect("cmd.ts 里应有 CmdReturn 接口");
    body.lines()
        .take_while(|line| line.trim() != "}")
        .filter_map(|line| {
            let t = line.trim();
            let (key, _) = t.split_once(':')?;
            let key = key.trim();
            if key.chars().all(|c| c.is_ascii_lowercase() || c == '_') && !key.is_empty() {
                Some(key.to_string())
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn command_rosters_stay_in_sync() {
    let dispatch = dispatch_commands();
    let frontend = frontend_commands();

    // 名单不能为空——解析失败（文件结构变了）必须炸，而不是静默通过
    assert!(
        dispatch.len() > 100,
        "分发表解析异常，只抓到 {} 条",
        dispatch.len()
    );
    assert!(
        frontend.len() > 100,
        "CmdReturn 解析异常，只抓到 {} 条",
        frontend.len()
    );

    let only_dispatch: Vec<_> = dispatch.difference(&frontend).collect();
    let only_frontend: Vec<_> = frontend.difference(&dispatch).collect();
    assert!(
        only_dispatch.is_empty() && only_frontend.is_empty(),
        "命令名单不同步（D8）：\n  只在分发表：{only_dispatch:?}\n  只在 cmd.ts：{only_frontend:?}\n\
         改命令时 desktop/web.rs 分发表与 src/lib/cmd.ts 必须一起改。"
    );
}
