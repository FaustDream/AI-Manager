//! 账号档案与登录态切换：客户端登录态 = Chromium 会话 + 票据，切换即
//! 「存当前账号档案 → 换入目标账号档案（票据按字节原样写回）」。

use crate::app::accounts::uid_of;
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::providers::{Provider, SwitchOutcome};
use crate::app::{fsx, paths, process};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub struct WorkBuddyProvider {
    pub intl: bool,
    /// `%LOCALAPPDATA%\CodeBuddyExtension\Data\Public\auth`
    auth_dir: Option<PathBuf>,
    /// 客户端数据目录，`~/.workbuddy` 或 `~/.workbuddy-ai`
    data_root: Option<PathBuf>,
    /// 账号档案根，默认 `<data>/profiles`
    pack_root: PathBuf,
    /// 需要检查是否在运行的进程名。测试可置空（否则本机开着客户端就跑不了切换用例）
    processes: &'static [&'static str],
}

impl WorkBuddyProvider {
    pub fn new(intl: bool) -> Self {
        Self {
            intl,
            auth_dir: default_auth_dir(),
            data_root: default_data_root(intl),
            pack_root: paths::data_dir().join("profiles"),
            // **只认本产品自己的 exe**，名字统一从 `app::products` 取。
            // `CodeBuddy CN.exe` / `CodeBuddy.exe` 属于 CodeBuddy IDE（另一个产品），
            // 混进来会让开着 IDE 的用户永远切不了号，报错还指错人。
            processes: if intl {
                crate::app::products::Product::WorkBuddyIntl.processes()
            } else {
                crate::app::products::Product::WorkBuddyCn.processes()
            },
        }
    }

    /// 指定全部路径（测试与迁移期用）
    pub fn at(intl: bool, auth_dir: PathBuf, data_root: PathBuf, pack_root: PathBuf) -> Self {
        let mut provider = Self::new(intl);
        provider.auth_dir = Some(auth_dir);
        provider.data_root = Some(data_root);
        provider.pack_root = pack_root;
        provider
    }

    /// 指定要检查的进程名（测试用空列表跳过「客户端是否在运行」这一步）
    pub fn with_processes(mut self, names: &'static [&'static str]) -> Self {
        self.processes = names;
        self
    }

    pub fn platform_key(&self) -> &'static str {
        "workbuddy"
    }

    pub fn edition(&self) -> &'static str {
        if self.intl {
            "intl"
        } else {
            "cn"
        }
    }

    pub fn file_name(&self) -> &'static str {
        if self.intl {
            "workbuddy-desktop-ai.info"
        } else {
            "workbuddy-desktop.info"
        }
    }

    pub fn process_names(&self) -> &'static [&'static str] {
        self.processes
    }

    pub fn live_ticket(&self) -> Option<PathBuf> {
        Some(self.auth_dir.as_ref()?.join(self.file_name()))
    }

    /// 客户端当前登录的 uid（**不**带 `@intl` 后缀）。
    pub fn current_uid(&self) -> Option<String> {
        let path = self.live_ticket()?;
        read_ticket_uid(&path)
    }

    /// 归属该版本的全部票据文件：主文件 + 客户端自己留的时间戳备份。
    /// 备份里带着当时的 `sessionState`，是「切回旧账号不弹登录页」的现成材料。
    pub(crate) fn ticket_files(&self) -> Vec<PathBuf> {
        let Some(dir) = self.auth_dir.as_ref().filter(|d| d.is_dir()) else {
            return Vec::new();
        };
        let base = self.file_name();
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .filter(|p| {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                // 同名主文件，或带时间戳/备份后缀的同族文件；`.json` 之类一律不碰
                name == base || (name.starts_with(base) && !name.ends_with(".json"))
            })
            .collect();
        // 主文件永远第一，备份按时间从新到旧——同一账号多份票据时取「当前登录的那份」
        let base = self.file_name();
        files.sort_by(|a, b| {
            let primary = |p: &PathBuf| p.file_name().and_then(|n| n.to_str()) != Some(base);
            primary(a).cmp(&primary(b)).then_with(|| {
                b.metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .cmp(&a.metadata().and_then(|m| m.modified()).ok())
            })
        });
        files
    }

    pub(crate) fn pack_dir(&self, uid: &str) -> PathBuf {
        self.pack_root.join(self.edition()).join(uid)
    }

    /// 档案是否可用（登录态 + 票据）。缺票据的档案切过去一定会停在登录页。
    pub fn pack_ready(&self, uid: &str) -> bool {
        self.pack_source(uid).is_some()
    }

    /// 可用的档案目录：正式目录优先；只剩 `.tmp` 时先把它认领回来。
    ///
    /// `.tmp` 是 `save_pack` 的中间产物。旧实现是「删掉正式目录 → rename tmp」，
    /// rename 一旦失败（客户端句柄未释放 / 杀软扫描），正式档案就消失了，
    /// 只留下一个内容完整的 tmp，而 `pack_ready` 只认正式目录——账号从此切不过去。
    /// 把 tmp 认领回来，是唯一既不丢数据、又能让这些账号恢复切换的办法。
    pub(crate) fn pack_source(&self, uid: &str) -> Option<PathBuf> {
        let dst = self.pack_dir(uid);
        if is_pack_complete(&dst) {
            return Some(dst);
        }
        let tmp = dst.with_extension("tmp");
        if !is_pack_complete(&tmp) {
            return None;
        }
        // 提升成正式目录；仍被占用就继续读 tmp，至少这一次切换能做
        if fsx::remove_path(&dst).is_ok() && std::fs::rename(&tmp, &dst).is_ok() {
            return Some(dst);
        }
        Some(tmp)
    }

    /// 把**当前**客户端登录态存成 uid 的档案（添加账号前调用，避免退出登录后档案丢失）。
    pub fn save_current_as_pack(&self, uid: &str) -> Result<PathBuf> {
        self.save_pack(uid)
    }

    /// 把**当前**客户端登录态存成 uid 的档案。
    pub(crate) fn save_pack(&self, uid: &str) -> Result<PathBuf> {
        let Some(root) = self.data_root.as_ref().filter(|p| p.exists()) else {
            return Err(AppError::new(
                ErrorKind::Io,
                "没有找到 WorkBuddy 的客户端数据目录",
            ));
        };
        let dst = self.pack_dir(uid);
        // 先写临时目录再整体换名：拷贝中途失败不会毁掉已有档案
        let tmp = dst.with_extension("tmp");
        fsx::remove_path(&tmp)?;
        std::fs::create_dir_all(&tmp)?;

        // 拷贝失败必须清掉半截 tmp：留着它下一次会被当成「完整档案」认领
        if let Err(e) = self.copy_pack_contents(root, &tmp, uid) {
            let _ = fsx::remove_path(&tmp);
            return Err(e);
        }

        // 三段式换名：dst → old，tmp → dst，成功再删 old。
        // 旧实现「先删 dst 再 rename」在任何一步失败时都会同时丢掉新旧两份档案。
        let old = dst.with_extension("old");
        let _ = fsx::remove_path(&old);
        let had_old = dst.exists();
        if had_old {
            if let Err(e) = std::fs::rename(&dst, &old) {
                let _ = fsx::remove_path(&tmp);
                return Err(AppError::new(
                    ErrorKind::Io,
                    format!("替换登录态档案失败（旧档案保留）：{e}"),
                ));
            }
        }
        if let Err(e) = std::fs::rename(&tmp, &dst) {
            let _ = fsx::remove_path(&tmp);
            if had_old {
                let _ = std::fs::rename(&old, &dst);
            }
            return Err(AppError::new(
                ErrorKind::Io,
                format!("替换登录态档案失败（已回滚到旧档案）：{e}"),
            ));
        }
        let _ = fsx::remove_path(&old);
        Ok(dst)
    }

    /// 拷贝会话 + storage + 票据到 `tmp`。抽出来是为了让调用方统一清掉半截目录。
    pub(crate) fn copy_pack_contents(&self, root: &Path, tmp: &Path, uid: &str) -> Result<()> {
        let session = root.join("app/session");
        if session.exists() {
            fsx::copy_profile_skipping_caches(&session, &tmp.join("app/session"))?;
        }
        // storage 下每个账号各有一份 user-<uid>-personal，仍是登录态相关，保持全量拷贝
        let storage = root.join("storage");
        if storage.exists() {
            fsx::copy_dir_merge(&storage, &tmp.join("storage"))?;
        }
        // 票据只在这个账号**自己**登录时才存：拿别人的票据存档，以后恢复就是「档案 A、票据 B」
        let ticket = self.live_ticket();
        let mut has_auth = false;
        if let Some(ticket) = ticket.as_ref().filter(|p| p.exists()) {
            if read_ticket_uid(ticket).as_deref() == Some(uid) {
                std::fs::create_dir_all(tmp)?;
                std::fs::copy(ticket, tmp.join("auth.info"))?;
                has_auth = true;
            }
        }
        let meta = json!({
            "uid": uid,
            "label": self.label(),
            "saved_at": chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "has_auth": has_auth,
        });
        std::fs::write(tmp.join("meta.json"), serde_json::to_vec_pretty(&meta)?)?;
        Ok(())
    }

    /// 把某账号的档案恢复到客户端数据目录。
    pub(crate) fn restore_pack(&self, uid: &str) -> Result<()> {
        let Some(root) = self.data_root.as_ref() else {
            return Err(AppError::new(
                ErrorKind::Io,
                "没有找到 WorkBuddy 的客户端数据目录",
            ));
        };
        let src = self.pack_source(uid).ok_or_else(|| {
            AppError::new(
                ErrorKind::SessionStateMissing,
                "本机没有这个账号完整的登录态档案。请先在客户端里登录一次该账号，\
                 关闭客户端后会自动补存，之后切换就不需要重新登录。",
            )
        })?;
        // 登录态是全局的，必须整体替换；storage 里有每个账号自己的 user-<uid>-personal，
        // 只能合并——整体替换会把其他账号的本地数据一起删掉。
        // 档案不含 Chromium 缓存，整体替换会顺带清掉上一个账号留下的缓存，客户端自己重建；
        // 留着反而可能把上个账号的私有 HTTP 响应带给新账号。
        fsx::replace_dir(&src.join("app/session"), &root.join("app/session"))?;
        if src.join("storage").is_dir() {
            fsx::copy_dir_merge(&src.join("storage"), &root.join("storage"))?;
        }
        // 票据按字节原样写回：sessionState 是服务端签发的，重建一个随机值客户端不认
        let ticket = self.live_ticket();
        if let Some(ticket) = ticket {
            let bytes = std::fs::read(src.join("auth.info"))?;
            fsx::write_atomic(&ticket, &bytes)?;
        }
        Ok(())
    }
}

/// 档案可用 = 会话目录 + 票据文件都在。目录名对不对由调用方负责。
fn is_pack_complete(dir: &Path) -> bool {
    dir.join("app/session").is_dir() && dir.join("auth.info").is_file()
}

fn read_ticket_uid(path: &Path) -> Option<String> {
    let text = std::fs::read(path).ok()?;
    let data: Value = serde_json::from_slice(&text).ok()?;
    data.pointer("/account/uid")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// 切换登录：存档当前账号 → 换入目标档案。
pub fn switch_to(
    provider: &WorkBuddyProvider,
    account_id: &str,
    name: &str,
) -> Result<SwitchOutcome> {
    let uid = uid_of(account_id);
    // 先判档案：缺档案时连客户端都不用关，直接告诉用户去登录一次
    if !provider.pack_ready(uid) {
        // 先判档案：没有档案就不要动当前登录态，一个字节都不写
        return Err(AppError::new(
            ErrorKind::SessionStateMissing,
            format!(
                "本机没有 {name} 完整的登录态档案，无法切换。档案（客户端会话 + 服务端签发的\
                 票据）只能来自客户端登录，网页登录的账号切不了。请在客户端里登录一次该账号\
                 （账号中心 → 添加账号 → 选 WorkBuddy → 登录失败时的「改用客户端登录」），\
                 档案会在那次流程里保存，之后切换就不需要重新登录。"
            ),
        ));
    }

    if provider.current_uid().as_deref() == Some(uid) {
        return Err(AppError::new(
            ErrorKind::Invalid,
            "这个账号就是客户端当前登录的账号，不需要切换",
        ));
    }
    if process::process_running(provider.process_names()) {
        return Err(AppError::new(
            ErrorKind::ClientRunning,
            format!("请先关闭 {} 客户端再切换", provider.label()),
        ));
    }

    // 1) 存档当前账号（不是目标账号才存；否则会用目标票据覆盖自己）
    let current = provider.current_uid();
    let mut notes = Vec::new();
    if let Some(current) = current.as_ref().filter(|c| *c != uid) {
        match provider.save_pack(current) {
            Ok(_) => notes.push("已存档当前账号的登录态，随时可以切回来".to_string()),
            // 存档失败不阻断切换：目标档案是完整的，用户的诉求是「切到目标账号」。
            // 但它是**部分失败**，必须按 Warn 落日志——以前跟着切换的成功一起记成
            // info，事后翻日志只会看到「切换成功」，看不出旧账号已经存不回来了。
            Err(e) => {
                crate::app::log::log(
                    crate::app::log::Level::Warn,
                    "providers",
                    format!(
                        "切换前存档当前账号 {current} 失败（切换继续）：{}",
                        e.message
                    ),
                );
                notes.push(format!("存档当前账号失败（切换继续）：{}", e.message));
            }
        }
    }

    // 2) 换入目标档案
    provider.restore_pack(uid)?;

    // 3) 验读：客户端认的是 .info 里的 uid，写回来对不上就等于没切
    if provider.current_uid().as_deref() != Some(uid) {
        return Err(AppError::new(
            ErrorKind::WriteFailed,
            "写入后校验失败：票据里的账号不是目标账号",
        ));
    }

    let mut message = format!("{} 已切换到 {name}", provider.label());
    if !notes.is_empty() {
        message.push_str(&format!("（{}）", notes.join("；")));
    }
    Ok(SwitchOutcome { message })
}

fn default_auth_dir() -> Option<PathBuf> {
    let local = std::env::var("LOCALAPPDATA").ok()?;
    Some(
        PathBuf::from(local)
            .join("CodeBuddyExtension")
            .join("Data")
            .join("Public")
            .join("auth"),
    )
}

fn default_data_root(intl: bool) -> Option<PathBuf> {
    let home = paths::home();
    Some(home.join(if intl { ".workbuddy-ai" } else { ".workbuddy" }))
}
