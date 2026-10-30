//! 各平台账号扫描。一家一个文件，扫描产物统一是 `ScannedAccount`。
//!
//! 共同约定：
//!   - 找不到客户端目录 = 返回 0，**不是错误**（没装就该没账号）
//!   - `source` 只留文件名，绝不落绝对路径（里面有 Windows 用户名）
//!   - 凭据走回调立刻入库，不收集成集合

pub mod qoder;

pub use qoder::{QoderProvider, ScannedAccount};
