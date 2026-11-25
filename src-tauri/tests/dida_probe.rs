//! 滴答清单真机探针：**会真的写入并删除任务**，默认 `#[ignore]`。
//!
//! ```text
//! cargo test --test dida_probe -- --ignored --nocapture
//! ```
//!
//! 回答两个只有真机能回答的问题：
//!   1. `parentId` 到底能不能通过开放平台 v1 写入？文档的 Task 定义里有
//!      「置空串解除父子关系」的语义，但 create/update 的 body 字段表里没列它。
//!      通了 → 界面启用真父子任务树；不通 → 只做 `items[]` 清单子任务，
//!      并把限制写进能力表，不假装支持。
//!   2. `POST /task/completed` 在国内版是否真的可用（旧代码注释说恒 404）。
//!
//! 探针在收件箱里建两条带唯一前缀的任务，跑完**全部删除**；
//! 不碰用户已有任务，不打印口令。

use ai_manager::app::db::Db;
use ai_manager::app::dida::{DidaItem, DidaTaskInput, Service};
use ai_manager::app::net::RealHttp;
use std::sync::Arc;

const MARK: &str = "AI-Manager-探针";

fn service() -> Service {
    Service::new(Arc::new(Db::open().expect("打开真实库")), Arc::new(RealHttp::new()))
}

#[tokio::test]
#[ignore = "会真的写入并删除滴答任务，手动执行"]
async fn probe_parent_id_and_completed_endpoint() {
    let service = service();
    let state = service.state().await;
    assert!(state.configured, "先在滴答清单页填好 API 口令再跑探针");
    let project = state
        .projects
        .iter()
        .find(|p| !p.closed)
        .expect("至少一个未关闭清单");
    println!("\n清单：{}（{}）", project.name, project.id);

    // ---- 1. items[] 清单子任务：文档明确支持，先确认基线
    let parent = service
        .save_task(DidaTaskInput {
            title: format!("{MARK} 父"),
            project_id: project.id.clone(),
            items: vec![DidaItem {
                title: "清单子项".into(),
                ..Default::default()
            }],
            tags: vec!["探针".into()],
            reminders: vec!["TRIGGER:-PT15M".into()],
            repeat_flag: Some("RRULE:FREQ=DAILY;INTERVAL=1".into()),
            ..Default::default()
        })
        .await
        .expect("建父任务");
    println!("items 回读：{} 条", parent.items.len());
    println!("tags 回读：{:?}", parent.tags);
    println!("reminders 回读：{:?}", parent.reminders);
    println!("repeatFlag 回读：{:?}", parent.repeat_flag);
    println!("kind 回读：{}", parent.kind);

    // ---- 2. parentId：建子任务时直接带父 id
    let child = service
        .save_task(DidaTaskInput {
            title: format!("{MARK} 子"),
            project_id: project.id.clone(),
            parent_id: Some(parent.id.clone()),
            ..Default::default()
        })
        .await
        .expect("建子任务");
    let create_honoured = child.parent_id == parent.id;
    println!("创建时带 parentId → 回读 parentId={:?}（{}）",
        child.parent_id,
        if create_honoured { "生效" } else { "被忽略" });

    // ---- 3. 更新时补挂 parentId（创建被忽略时这条仍可能生效）
    let updated = service
        .save_task(DidaTaskInput {
            id: child.id.clone(),
            title: format!("{MARK} 子"),
            project_id: project.id.clone(),
            parent_id: Some(parent.id.clone()),
            ..Default::default()
        })
        .await
        .expect("更新子任务");
    let update_honoured = updated.parent_id == parent.id;
    println!("更新时带 parentId → 回读 parentId={:?}（{}）",
        updated.parent_id,
        if update_honoured { "生效" } else { "被忽略" });

    // ---- 4. 列表里父子关系是否可见
    let listed = service.list_tasks(&project.id).await.expect("读清单任务");
    let listed_child = listed.iter().find(|t| t.id == child.id);
    println!(
        "列表里的子任务 parentId={:?}",
        listed_child.map(|t| t.parent_id.clone())
    );

    // ---- 5. 已完成端点可用性
    service
        .complete_task(&project.id, &parent.id)
        .await
        .expect("完成父任务");
    let now = chrono::Local::now();
    let from = (now - chrono::Duration::days(1))
        .format("%Y-%m-%dT%H:%M:%S%z")
        .to_string();
    let to = (now + chrono::Duration::days(1))
        .format("%Y-%m-%dT%H:%M:%S%z")
        .to_string();
    match service
        .completed_tasks(std::slice::from_ref(&project.id), &from, &to)
        .await
    {
        Ok(done) => println!(
            "已完成端点可用：{} 条（含探针父任务：{}）",
            done.len(),
            done.iter().any(|t| t.id == parent.id)
        ),
        Err(e) => println!("已完成端点不可用：{} / {}", e.kind.as_str(), e.message),
    }

    // ---- 收尾：探针建的任务全部删掉，不留垃圾
    for id in [&child.id, &parent.id] {
        match service.delete_task(&project.id, id).await {
            Ok(()) => println!("已清理 {id}"),
            Err(e) => println!("清理 {id} 失败（请手动删除 {MARK} 任务）：{}", e.message),
        }
    }

    println!(
        "\n结论：真父子任务 {}；清单子任务 {} 条",
        if create_honoured || update_honoured { "可用" } else { "不可用，只能做 items[] 清单子任务" },
        parent.items.len()
    );
}