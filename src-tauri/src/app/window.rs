//! 仪表盘各区块共用的时间窗口。
//!
//! 积分收支、积分趋势、Token 用量、会话统计四个命令都只认界面给的 `from` / `to`：
//! 闭区间、按本地时区取整天。**不设天数上限**——区间由用户决定，后端不替用户
//! 划定「最多能看多久」。
//!
//! 缺省（手工调用未传）回落最近 14 天；`from` 晚于 `to` 时自动对调，
//! 日期控件被手输成倒序也照样出图。

use chrono::{Local, NaiveDate, TimeZone};

/// 缺省窗口的跨度（天）：没传日期时用它兜底。
const DEFAULT_DAYS: i64 = 14;

/// 闭区间 `[from, to]`，两端都是本地日期的整天。
#[derive(Debug, Clone, Copy)]
pub struct DayWindow {
    pub from: NaiveDate,
    pub to: NaiveDate,
}

impl DayWindow {
    /// 从 IPC 的 `from` / `to`（`YYYY-MM-DD`）解析；非法或缺失走默认值。
    pub fn parse(from: Option<&str>, to: Option<&str>) -> Self {
        let today = Local::now().date_naive();
        let from = from
            .and_then(parse_day)
            .unwrap_or(today - chrono::Duration::days(DEFAULT_DAYS - 1));
        let to = to.and_then(parse_day).unwrap_or(today);
        let (from, to) = if from > to { (to, from) } else { (from, to) };
        Self { from, to }
    }

    /// 只按天数构造（今天往前 `days` 天，含今天）。
    pub fn last_days(days: i64) -> Self {
        let today = Local::now().date_naive();
        Self {
            from: today - chrono::Duration::days(days.clamp(1, i64::MAX) - 1),
            to: today,
        }
    }

    pub fn from_iso(&self) -> String {
        self.from.format("%Y-%m-%d").to_string()
    }

    pub fn to_iso(&self) -> String {
        self.to.format("%Y-%m-%d").to_string()
    }

    /// 窗口起点（`from` 当天 00:00:00）的毫秒时间戳。
    pub fn start_ms(&self) -> i64 {
        at_ms(self.from, 0, 0, 0)
    }

    /// 窗口终点（`to` 当天 23:59:59.999）的毫秒时间戳，闭区间。
    pub fn end_ms(&self) -> i64 {
        at_ms(self.to, 23, 59, 59) + 999
    }
}

fn parse_day(text: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d").ok()
}

fn at_ms(day: NaiveDate, hour: u32, min: u32, sec: u32) -> i64 {
    Local
        .from_local_datetime(&day.and_hms_opt(hour, min, sec).expect("时分秒合法"))
        .single()
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_iso_bounds() {
        let window = DayWindow::parse(Some("2026-03-01"), Some("2026-03-31"));
        assert_eq!(window.from_iso(), "2026-03-01");
        assert_eq!(window.to_iso(), "2026-03-31");
        assert!(window.start_ms() < window.end_ms());
    }

    #[test]
    fn swaps_reversed_bounds() {
        let window = DayWindow::parse(Some("2026-03-31"), Some("2026-03-01"));
        assert_eq!(window.from_iso(), "2026-03-01");
        assert_eq!(window.to_iso(), "2026-03-31");
    }

    #[test]
    fn falls_back_to_recent_days_on_garbage() {
        let window = DayWindow::parse(Some("不是日期"), None);
        assert_eq!(
            window.to_iso(),
            Local::now().date_naive().format("%Y-%m-%d").to_string()
        );
        assert!(window.from <= window.to);
    }
}
