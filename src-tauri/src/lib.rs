// 库入口：契约层（contracts/）与各层模块。契约编号定义见 AGENTS.md §4。

pub mod app;
pub mod contracts;
pub mod desktop;
pub mod types;

pub use app::clock::{Clock, FakeClock, SystemClock};
pub use app::error::AppError;
pub use app::net::{Http, HttpResponse, MockHttp};
pub use types::*;
