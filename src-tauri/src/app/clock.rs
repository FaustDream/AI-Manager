//! 时钟注入。契约 2/3/4/6 全依赖时间边界，不注入就没法测。
//!
//! CI 检查：`src-tauri/src/capabilities/` 下出现 `Local::now()` / `Utc::now()` 即失败。

use chrono::{DateTime, Local, NaiveDate, TimeZone};
use std::sync::Mutex;

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Local>;
    fn today(&self) -> NaiveDate {
        self.now().date_naive()
    }
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Local> {
        Local::now()
    }
}

/// 测试用时钟：可拨到任意时刻，也可向前推进。
pub struct FakeClock {
    at: Mutex<DateTime<Local>>,
}

impl FakeClock {
    pub fn at(rfc3339: &str) -> Self {
        let fixed = DateTime::parse_from_rfc3339(rfc3339)
            .unwrap_or_else(|e| panic!("FakeClock 时间格式错误 {rfc3339}: {e}"))
            .with_timezone(&Local);
        Self {
            at: Mutex::new(fixed),
        }
    }

    pub fn set(&self, rfc3339: &str) {
        let fixed = DateTime::parse_from_rfc3339(rfc3339)
            .expect("FakeClock 时间格式错误")
            .with_timezone(&Local);
        *self.at.lock().unwrap() = fixed;
    }

    pub fn advance(&self, hours: i64) {
        let mut guard = self.at.lock().unwrap();
        *guard += chrono::Duration::hours(hours);
    }
}

impl Clock for FakeClock {
    fn now(&self) -> DateTime<Local> {
        *self.at.lock().unwrap()
    }
}

/// 按本地日期构造一个时刻，测试里写时间比写 RFC3339 短得多。
pub fn local(year: i32, month: u32, day: u32, hour: u32, min: u32) -> DateTime<Local> {
    Local
        .with_ymd_and_hms(year, month, day, hour, min, 0)
        .single()
        .expect("本地时间不合法")
}
