// P0 阶段：只有类型与注入点，全部业务实现为 todo!()，测试必然全红。
// 契约编号对应 ARCHITECTURE-v2.md §7。

pub mod app;
pub mod contracts;
pub mod desktop;
pub mod types;

pub use app::clock::{Clock, FakeClock, SystemClock};
pub use app::error::AppError;
pub use app::net::{Http, HttpResponse, MockHttp};
pub use types::*;
