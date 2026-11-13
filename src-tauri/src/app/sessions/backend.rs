// 平台会话库的适配器接缝（架构评审 C2）：彻底删除 / 预览两个动作按 kind
// 分派的唯一分支点，平台显示名也收在这里。各平台实现仍在 delete.rs / preview.rs
// （写前 checkpoint、正文清理等纪律在那里），本文件只收敛分派——
// 新增平台 = 加一个实现块，不再同步多处 match。

use crate::app::clients::Roots;
use crate::app::error::Result;
use crate::app::sessions::delete;
use crate::app::sessions::preview::{self, Preview, Target};

pub(crate) trait SessionBackend {
    /// 永久删除：返回（删除的文件数, 释放的字节数）。失败必须让调用方记进报告。
    fn purge(&self, roots: &Roots, target: &Target) -> Result<(usize, i64)>;
    fn load_preview(&self, roots: &Roots, target: &Target) -> Preview;
}

struct Workbuddy;
struct Codebuddy;
struct Qoder;
struct Zcode;

static WORKBUDDY: Workbuddy = Workbuddy;
static CODEBUDDY: Codebuddy = Codebuddy;
static QODER: Qoder = Qoder;
static ZCODE: Zcode = Zcode;

impl SessionBackend for Workbuddy {
    fn purge(&self, roots: &Roots, target: &Target) -> Result<(usize, i64)> {
        delete::purge_workbuddy(roots, target)
    }
    fn load_preview(&self, roots: &Roots, target: &Target) -> Preview {
        preview::load_workbuddy(roots, target)
    }
}

impl SessionBackend for Codebuddy {
    fn purge(&self, roots: &Roots, target: &Target) -> Result<(usize, i64)> {
        delete::purge_codebuddy(roots, target)
    }
    fn load_preview(&self, roots: &Roots, target: &Target) -> Preview {
        preview::load_codebuddy(roots, target)
    }
}

impl SessionBackend for Qoder {
    fn purge(&self, roots: &Roots, target: &Target) -> Result<(usize, i64)> {
        delete::purge_sqlite(roots, target, "qoder")
    }
    fn load_preview(&self, roots: &Roots, target: &Target) -> Preview {
        preview::load_qoder(roots, target)
    }
}

impl SessionBackend for Zcode {
    fn purge(&self, roots: &Roots, target: &Target) -> Result<(usize, i64)> {
        delete::purge_sqlite(roots, target, "zcode")
    }
    fn load_preview(&self, roots: &Roots, target: &Target) -> Preview {
        preview::load_zcode(roots, target)
    }
}

/// kind（索引行记录的来源）→ 平台后端。Trae 是云端占位、没有本地库，不进这张表，
/// 它的删除是墓碑、预览是占位文案，留在各自调用点。
pub(crate) fn backend_for(kind: &str) -> Option<&'static dyn SessionBackend> {
    match kind {
        "workbuddy" => Some(&WORKBUDDY),
        "codebuddy" => Some(&CODEBUDDY),
        "qoder" => Some(&QODER),
        "zcode" => Some(&ZCODE),
        _ => None,
    }
}

/// 客户端平台键 → 显示名（关客户端报告、日志用）。
/// WorkBuddy 两版的名字统一由 `app::products` 给，避免这里和账号中心各叫各的。
pub(crate) fn label(platform: &str) -> String {
    if let Some(product) = crate::app::products::Product::from_account_key(platform) {
        return product.label().to_string();
    }
    match platform {
        "qoder" => "Qoder CN".into(),
        "qoder_intl" => "Qoder 国际版".into(),
        "zcode" => "ZCode".into(),
        other => other.to_string(),
    }
}
