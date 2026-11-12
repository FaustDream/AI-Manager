//! 应用配置：`data/config.toml` 是全局配置的唯一真源。
//!
//! 两条调整通道，一个存储：
//! - **改文件**：启动时加载一次，改完重启生效；
//! - **设置界面**：`config_set` 更新内存并写回文件，立即生效（文件注释会被重写掉）。
//!
//! 代码里不再散落硬编码的阈值/间隔；确实无法配置化的（平台端点、协议常量）
//! 在使用处注释说明。历史遗留的 kv 配置键（checkin.max_concurrent 等）在首次
//! 生成配置文件时迁移进来并删除，老用户调过的值不丢。

use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::{fsx, paths};
use serde::{Deserialize, Serialize};
use std::sync::RwLock;

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CheckinConfig {
    /// 批量签到/诊断的并发上限（实际执行时收敛到 1..=6）
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub max_concurrent: usize,
    /// 账号之间的间隔毫秒（实际执行时上限 60000）
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub gap_ms: u64,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkConfig {
    /// 单次平台请求的超时秒数
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub timeout_secs: u64,
    /// 只读 GET 失败后的重试次数（POST 永不重试：签到重放等于重复领取）
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub get_retry: u32,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UsageConfig {
    /// 用量自动采集的间隔分钟（采集器只读客户端本地文件）
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub scan_interval_min: u64,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ActivityConfig {
    /// 每个账号保留的本机旅行记录条数
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub travel_events_limit: usize,
    /// ZCode 活动自动领取（领取要开浏览器过验证码，缺省关）
    pub zcode_auto_claim: bool,
}

/// 点主窗口关闭按钮时的行为。存配置文件而不是 kv：它是运行行为，
/// 和签到并发、超时一样属于「应用怎么跑」，改完要能被 `data/config.toml` 解释。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseAction {
    /// 收进系统托盘，进程常驻（定时任务继续跑）
    #[default]
    Tray,
    /// 直接退出进程
    Exit,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowConfig {
    pub close_action: CloseAction,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DashboardConfig {
    /// 积分数据超过多少分钟没刷新，仪表盘就自动补刷一轮
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub stale_refresh_min: u64,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    #[serde(default)]
    pub checkin: CheckinConfig,
    #[serde(default)]
    pub network: NetworkConfig,
    #[serde(default)]
    pub usage: UsageConfig,
    #[serde(default)]
    pub activity: ActivityConfig,
    #[serde(default)]
    pub dashboard: DashboardConfig,
    #[serde(default)]
    pub window: WindowConfig,
}

impl Default for WindowConfig {
    fn default() -> Self {
        // 默认收托盘：定时签到靠常驻进程跑，一关就退等于把自动签到关掉
        Self {
            close_action: CloseAction::Tray,
        }
    }
}

impl Default for CheckinConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 3,
            gap_ms: 0,
        }
    }
}
impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            timeout_secs: 15,
            get_retry: 1,
        }
    }
}
impl Default for UsageConfig {
    fn default() -> Self {
        Self {
            scan_interval_min: 30,
        }
    }
}
impl Default for ActivityConfig {
    fn default() -> Self {
        Self {
            travel_events_limit: 50,
            zcode_auto_claim: false,
        }
    }
}
impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            stale_refresh_min: 10,
        }
    }
}

/// 文件头说明：字段级注释写在首次生成的模板里，设置界面保存后会重写正文，
/// 这段头一直保留，说明两条调整通道的规则。
const HEADER: &str = r#"
# AI Manager 应用配置（data/config.toml）
#
# - 改这个文件后重启软件生效；
# - 设置界面的「配置」区改完立即生效，同时会写回本文件（字段注释会被去掉）；
# - 缺字段一律用内置默认值，删掉某行等于恢复默认。
#
"#;

/// 首次生成用的带注释模板：值 = 内置默认。之后以文件实际内容为准。
const TEMPLATE: &str = r#"
[checkin]
# 批量签到/诊断的并发上限（1-6）
max_concurrent = 3
# 账号之间的间隔毫秒（0-60000）
gap_ms = 0

[network]
# 单次平台请求的超时秒数；走代理调慢链路时可以调大
timeout_secs = 15
# 只读 GET 失败后的重试次数（POST 永不重试）
get_retry = 1

[usage]
# 用量自动采集的间隔分钟
scan_interval_min = 30

[activity]
# 每个账号保留的本机旅行记录条数
travel_events_limit = 50
# ZCode 活动自动领取（要开浏览器过验证码，缺省关）
zcode_auto_claim = false

[dashboard]
# 积分数据多少分钟没刷新就自动补刷一轮
stale_refresh_min = 10

[window]
# 点关闭按钮：tray = 收进托盘继续常驻；exit = 直接退出
close_action = "tray"
"#;

static CONFIG: RwLock<Option<AppConfig>> = RwLock::new(None);

/// 进程内当前配置。未初始化（单元测试）时给默认值，业务代码不用判空。
pub fn get() -> AppConfig {
    CONFIG
        .read()
        .ok()
        .and_then(|guard| guard.clone())
        .unwrap_or_default()
}

/// 更新内存并写回文件（设置界面通道）。文件写入失败也保留内存值：
/// 界面已经生效，落盘失败下次启动回到文件值，日志里有据可查。
pub fn store(config: AppConfig) -> Result<()> {
    *CONFIG
        .write()
        .map_err(|e| AppError::new(ErrorKind::Io, format!("配置锁损坏：{e}")))? =
        Some(config.clone());
    write_file(&config)
}

/// 启动时调用：加载 `data/config.toml`（缺文件先迁移旧 kv 键再落模板）。
/// 解析失败不崩：记日志、用默认值——一份坏配置不该挡住整个应用。
pub fn init(db: &crate::app::db::Db) -> AppConfig {
    let path = paths::data_dir().join("config.toml");
    let (config, existed) = match std::fs::read_to_string(&path) {
        Ok(text) => match toml::from_str::<AppConfig>(&text) {
            Ok(config) => (config, true),
            Err(e) => {
                crate::app::log::log(
                    crate::app::log::Level::Warn,
                    "config",
                    format!(
                        "config.toml 解析失败（{}），已用默认值；修好文件后重启即可",
                        e
                    ),
                );
                (AppConfig::default(), true)
            }
        },
        Err(_) => {
            let config = migrate_from_kv(db);
            (config, false)
        }
    };
    if !existed {
        // 全新安装（值等于默认）才写带字段注释的模板；迁移过旧 kv 值的按实际值落盘
        if config == AppConfig::default() {
            let _ = fsx::write_atomic(&path, TEMPLATE.as_bytes());
        } else {
            let _ = write_file(&config);
        }
    }
    if let Ok(mut guard) = CONFIG.write() {
        *guard = Some(config.clone());
    }
    config
}

/// 旧 kv 配置键 → 配置对象（只在首次生成配置文件时执行一次），迁移完删掉旧键。
fn migrate_from_kv(db: &crate::app::db::Db) -> AppConfig {
    let kv_number = |key: &str| -> Option<i64> {
        db.kv_get(key)
            .ok()
            .flatten()
            .and_then(|text| text.trim().parse().ok())
    };
    let kv_bool = |key: &str| -> Option<bool> {
        db.kv_get(key)
            .ok()
            .flatten()
            .and_then(|text| serde_json::from_str::<bool>(&text).ok())
    };
    let config = AppConfig {
        checkin: CheckinConfig {
            max_concurrent: kv_number("checkin.max_concurrent")
                .map(|v| v as usize)
                .unwrap_or(3),
            gap_ms: kv_number("checkin.gap_ms").map(|v| v as u64).unwrap_or(0),
        },
        activity: ActivityConfig {
            zcode_auto_claim: kv_bool("zcode_auto_claim").unwrap_or(false),
            ..ActivityConfig::default()
        },
        ..AppConfig::default()
    };
    for key in [
        "checkin.max_concurrent",
        "checkin.gap_ms",
        "zcode_auto_claim",
    ] {
        let _ = db.kv_delete(key);
    }
    config
}

fn write_file(config: &AppConfig) -> Result<()> {
    let path = paths::data_dir().join("config.toml");
    let body = toml::to_string_pretty(config)
        .map_err(|e| AppError::new(ErrorKind::Invalid, format!("配置序列化失败：{e}")))?;
    std::fs::create_dir_all(paths::data_dir())
        .map_err(|e| AppError::new(ErrorKind::Io, format!("创建数据目录失败：{e}")))?;
    fsx::write_atomic(&path, format!("{HEADER}{body}").as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let config: AppConfig = toml::from_str("[network]\ntimeout_secs = 42").unwrap();
        assert_eq!(config.network.timeout_secs, 42);
        assert_eq!(config.network.get_retry, 1, "缺字段应回默认");
        assert_eq!(config.checkin.max_concurrent, 3);
    }

    #[test]
    fn broken_file_is_reported_not_defaults_silently() {
        // 解析失败要报错（调用方记日志降级），而不是静默吞成默认值
        assert!(toml::from_str::<AppConfig>("%%%不是toml%%%").is_err());
    }

    #[test]
    fn roundtrip_preserves_values() {
        let config = AppConfig {
            network: NetworkConfig {
                timeout_secs: 33,
                get_retry: 2,
            },
            ..AppConfig::default()
        };
        let text = toml::to_string_pretty(&config).unwrap();
        let parsed: AppConfig = toml::from_str(&text).unwrap();
        assert_eq!(parsed.network.timeout_secs, 33);
        assert_eq!(parsed.activity.travel_events_limit, 50);
    }

    #[test]
    fn template_parses_and_matches_defaults() {
        let parsed: AppConfig = toml::from_str(TEMPLATE).unwrap();
        assert_eq!(parsed, AppConfig::default(), "模板值必须与内置默认一致");
    }

    #[test]
    fn migrate_reads_legacy_kv_and_cleans_up() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = crate::app::db::Db::open_at(&dir.path().join("app.db")).unwrap();
        db.kv_set("checkin.max_concurrent", "5").unwrap();
        db.kv_set("zcode_auto_claim", "true").unwrap();

        let config = migrate_from_kv(&db);
        assert_eq!(config.checkin.max_concurrent, 5);
        assert!(config.activity.zcode_auto_claim);
        assert!(
            db.kv_get("checkin.max_concurrent").unwrap().is_none(),
            "迁移后旧键应删除"
        );
    }
}
