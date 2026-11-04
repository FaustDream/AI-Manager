// 平台会话库的适配器接缝（架构评审 C2）：删除 / 恢复 / 预览三个动作按 kind
// 分派的唯一分支点，平台显示名也收在这里。各平台实现仍在 delete.rs / preview.rs
// （写前 checkpoint、正文导出等纪律在那里），本文件只收敛分派——
// 新增平台 = 加一个实现块，不再同步 3-4 处 match。

use crate::app::clients::Roots;
use crate::app::error::Result;
use crate::app::sessions::delete;
use crate::app::sessions::preview::{self, Preview, Target};
use crate::app::sessions::trash::Item;
use std::path::Path;

pub(crate) trait SessionBackend {
    fn delete(
        &self,
        roots: &Roots,
        target: &Target,
        batch: &Path,
        title: &str,
        partials: &mut Vec<Item>,
    ) -> Result<Item>;
    fn restore(&self, roots: &Roots, dir: &Path, item: &Item) -> Result<()>;
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
    fn delete(
        &self,
        roots: &Roots,
        target: &Target,
        batch: &Path,
        title: &str,
        partials: &mut Vec<Item>,
    ) -> Result<Item> {
        delete::delete_workbuddy(roots, target, batch, title, partials)
    }
    fn restore(&self, roots: &Roots, dir: &Path, item: &Item) -> Result<()> {
        delete::restore_workbuddy(roots, dir, item)
    }
    fn load_preview(&self, roots: &Roots, target: &Target) -> Preview {
        preview::load_workbuddy(roots, target)
    }
}

impl SessionBackend for Codebuddy {
    fn delete(
        &self,
        roots: &Roots,
        target: &Target,
        batch: &Path,
        title: &str,
        partials: &mut Vec<Item>,
    ) -> Result<Item> {
        delete::delete_codebuddy(roots, target, batch, title, partials)
    }
    fn restore(&self, roots: &Roots, dir: &Path, item: &Item) -> Result<()> {
        delete::restore_codebuddy(roots, dir, item)
    }
    fn load_preview(&self, roots: &Roots, target: &Target) -> Preview {
        preview::load_codebuddy(roots, target)
    }
}

impl SessionBackend for Qoder {
    fn delete(
        &self,
        roots: &Roots,
        target: &Target,
        batch: &Path,
        title: &str,
        partials: &mut Vec<Item>,
    ) -> Result<Item> {
        delete::delete_sqlite(roots, target, batch, title, "qoder", partials)
    }
    fn restore(&self, roots: &Roots, dir: &Path, item: &Item) -> Result<()> {
        delete::restore_sqlite(roots, dir, item, "qoder")
    }
    fn load_preview(&self, roots: &Roots, target: &Target) -> Preview {
        preview::load_qoder(roots, target)
    }
}

impl SessionBackend for Zcode {
    fn delete(
        &self,
        roots: &Roots,
        target: &Target,
        batch: &Path,
        title: &str,
        partials: &mut Vec<Item>,
    ) -> Result<Item> {
        delete::delete_sqlite(roots, target, batch, title, "zcode", partials)
    }
    fn restore(&self, roots: &Roots, dir: &Path, item: &Item) -> Result<()> {
        delete::restore_sqlite(roots, dir, item, "zcode")
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
pub(crate) fn label(platform: &str) -> String {
    match platform {
        "workbuddy" => "WorkBuddy 国内版".into(),
        "workbuddy_intl" => "WorkBuddy 国际版".into(),
        "qoder" => "Qoder CN".into(),
        "qoder_intl" => "Qoder 国际版".into(),
        "zcode" => "ZCode".into(),
        other => other.to_string(),
    }
}
