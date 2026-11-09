//! 进程内事件总线：后端 → 前端的实时通知，**唯一发入口**。
//!
//! 为什么不让业务各处直接 `AppHandle::emit`：桌面壳（WebView emit）与网页壳
//! （SSE）要共用同一条事件路径，业务只管往总线发，壳各自决定怎么送。
//! 桌面壳的转发在 `desktop::background::spawn_event_bridges`，网页壳在
//! `desktop::web` 的 `/events`。
//!
//! ARCH-12 白名单成员：与 DB / 窗口无关、任意调用深度都要拿，允许 static。
//! 事件是「刷新提示」不是可靠队列：没有订阅者或订阅端跟不上都直接丢。

use serde::Serialize;
use std::sync::OnceLock;
use tokio::sync::broadcast;

/// 总线上的一条事件：名字与前端 `listen` 的事件名一致（如 `checkin:round`）。
/// Clone 是 SSE 流（BroadcastStream）对条目的要求。
#[derive(Clone)]
pub struct Event {
    pub name: String,
    pub payload: serde_json::Value,
}

fn channel() -> &'static broadcast::Sender<Event> {
    static CHANNEL: OnceLock<broadcast::Sender<Event>> = OnceLock::new();
    // 容量给到一轮签到的逐账号事件量级；溢出走 Lagged（调用方跳过即可）
    CHANNEL.get_or_init(|| broadcast::channel(256).0)
}

/// 发布一条事件。序列化失败退成 null——事件只携带刷新信号，不携带关键数据。
pub fn emit(name: &str, payload: impl Serialize) {
    let event = Event {
        name: name.to_string(),
        payload: serde_json::to_value(payload).unwrap_or_default(),
    };
    let _ = channel().send(event);
}

/// 订阅事件流。关闭 Receiver 即退订；`Lagged` 由调用方按「跳过」处理。
pub fn subscribe() -> broadcast::Receiver<Event> {
    channel().subscribe()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn subscribers_receive_emitted_events() {
        let mut rx1 = subscribe();
        let mut rx2 = subscribe();

        emit("test:hub", serde_json::json!({ "n": 1 }));

        // 全局通道是共享的：并行测试里可能夹带其他测试的事件，只认自己的
        for rx in [&mut rx1, &mut rx2] {
            loop {
                let event = rx.recv().await.expect("订阅者应收到事件");
                if event.name != "test:hub" {
                    continue;
                }
                assert_eq!(event.payload["n"], 1);
                break;
            }
        }
    }

    #[tokio::test]
    async fn emit_without_subscribers_is_silently_dropped() {
        // 没有任何订阅者：send 返回 Err，但 emit 必须不 panic
        emit("test:no-listener", "nothing");
    }
}
