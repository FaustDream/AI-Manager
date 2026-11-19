//! 批量动作的实时进度：界面按钮旁「12 / 40」的唯一来源。
//!
//! 走的是既有事件总线（`app::events`），**不新开命令面**——进度是提示而非契约，
//! 前端订阅 `progress` 事件自行累计即可；订阅端没起或跟不上，丢了也只是一次少显示。
//!
//! 为什么总数由后端给、完成数由前端数：总数只有发起批次的地方知道（过滤完停用账号后
//! 剩几个要调接口），而完成数每处理完一个就能推一条，前端累加即可，不必回写状态。

use crate::app::events;
use serde_json::json;

const EVENT: &str = "progress";

/// 批次开始：`total` 是真正会被处理的数量（已剔除停用/免签）。
pub fn start(action: &str, batch: &str, total: usize) {
    events::emit(
        EVENT,
        json!({ "action": action, "batch": batch, "phase": "start", "total": total }),
    );
}

/// 处理完一项。`outcome` 取 `ok` / `skip` / `fail`（与日志的 Outcome 同口径）。
pub fn tick(action: &str, batch: &str, outcome: &str) {
    events::emit(
        EVENT,
        json!({ "action": action, "batch": batch, "phase": "tick", "outcome": outcome }),
    );
}

/// 批次结束：前端据此把进度条收掉，而不是等按钮的 loading 自己停。
pub fn end(action: &str, batch: &str) {
    events::emit(
        EVENT,
        json!({ "action": action, "batch": batch, "phase": "end" }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::events::subscribe;

    #[tokio::test]
    async fn start_tick_end_carry_action_and_batch() {
        let mut rx = subscribe();
        start("t_action", "t_batch", 4);
        tick("t_action", "t_batch", "ok");
        end("t_action", "t_batch");

        let mut seen = Vec::new();
        // 全局通道共享：只认自己的批次，其他测试夹带的事件跳过
        while seen.len() < 3 {
            let event = rx.recv().await.expect("订阅者应收到进度事件");
            if event.name != EVENT || event.payload["batch"] != "t_batch" {
                continue;
            }
            seen.push(event.payload);
        }
        assert_eq!(seen[0]["phase"], "start");
        assert_eq!(seen[0]["total"], 4);
        assert_eq!(seen[1]["phase"], "tick");
        assert_eq!(seen[1]["outcome"], "ok");
        assert_eq!(seen[2]["phase"], "end");
        // 三个事件都必须带 action：界面按动作键找「这条进度是哪颗按钮的」
        for payload in &seen {
            assert_eq!(payload["action"], "t_action");
        }
    }
}
