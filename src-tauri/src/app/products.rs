//! 四个「长得像」的产品的身份表——它们之间的唯一事实来源。
//!
//! 这四个东西经常被搞混，而混淆的代价不是显示难看，是**动错产品**：
//!
//! | 产品 | 进程 | 数据目录 | 票据 / 凭据 |
//! | --- | --- | --- | --- |
//! | WorkBuddy 国内版 | `WorkBuddy.exe` | `~/.workbuddy` | `workbuddy-desktop.info` |
//! | WorkBuddy 国际版 | `WorkBuddyAI.exe` | `~/.workbuddy-ai` | `workbuddy-desktop-ai.info` |
//! | CodeBuddy IDE | `CodeBuddy CN.exe` / `CodeBuddy.exe` | `%APPDATA%\CodeBuddy CN` | state.vscdb 里的 `secret://` 条目 |
//! | CodeBuddy 插件 | JetBrains 宿主（`idea64.exe` …） | `%APPDATA%\JetBrains\*` | `secret-storage.xml` |
//!
//! 已经踩过的两个坑都记在这里：
//!   1. WorkBuddy 国内版的进程名表里混进 `CodeBuddy CN.exe`——用户开着 IDE 就永远切不了号，
//!      报错还是「请先关闭 WorkBuddy 客户端」，关掉 WorkBuddy 也没用；
//!   2. `idea64` 少了 `.exe`——守卫永远认为 IDEA 没开，写进去被 IDE 退出时回写覆盖。
//!
//! 规矩只有一条：**任何地方要判断「这是哪个产品」「它叫什么」「它跑起来是什么进程」
//! 「它装在哪」，都从这里取，不准再就地写字符串。** 新增产品 = 加一行，
//! `tests::products_never_overlap` 会盯着不许串。

/// 四个产品。顺序 = 界面上的展示顺序。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Product {
    WorkBuddyCn,
    WorkBuddyIntl,
    CodeBuddyIde,
    CodeBuddyIdea,
}

/// 安装位置候选的形态。真实路径由 `accounts::add::candidates` 按环境变量展开。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Install {
    /// 固定绝对路径（便携版 / 本机约定）
    Abs(&'static str),
    /// `%LOCALAPPDATA%\Programs\<dir>\<exe>`
    UserPrograms(&'static str, &'static str),
    /// `%ProgramFiles%\<dir>\<exe>`
    ProgramFiles(&'static str, &'static str),
}

/// JetBrains 产品目录名（小写前缀）→ 宿主进程词干。
///
/// 插件装在哪个 IDE 里，凭据就写在那个 IDE 的 `secret-storage.xml`；
/// 关不关它，也只能由「装了插件的那个 IDE」决定，不能一律拿 `idea64.exe` 去猜。
pub const JETBRAINS_HOSTS: &[(&str, &str)] = &[
    ("intellijidea", "idea"),
    ("pycharm", "pycharm"),
    ("goland", "goland"),
    ("webstorm", "webstorm"),
    ("phpstorm", "phpstorm"),
    ("rubymine", "rubymine"),
    ("clion", "clion"),
    ("rider", "rider"),
    ("datagrip", "datagrip"),
    ("dataspell", "dataspell"),
    ("aqua", "aqua"),
    ("androidstudio", "studio"),
];

impl Product {
    pub const ALL: [Product; 4] = [
        Product::WorkBuddyCn,
        Product::WorkBuddyIntl,
        Product::CodeBuddyIde,
        Product::CodeBuddyIdea,
    ];

    /// 界面与日志里的**唯一**显示名。两处叫法不同就是下一次混淆的开始。
    pub fn label(self) -> &'static str {
        match self {
            Product::WorkBuddyCn => "WorkBuddy 国内版",
            Product::WorkBuddyIntl => "WorkBuddy 国际版",
            Product::CodeBuddyIde => "CodeBuddy IDE",
            Product::CodeBuddyIdea => "CodeBuddy 插件",
        }
    }

    /// 账号平台键（`accounts` / `add` / `checkin` 用的那套）。CodeBuddy 系不是账号平台。
    pub fn account_key(self) -> Option<&'static str> {
        match self {
            Product::WorkBuddyCn => Some("workbuddy"),
            Product::WorkBuddyIntl => Some("workbuddy_intl"),
            _ => None,
        }
    }

    /// 切换命令的目标键（`account_switch_codebuddy` 的 target 参数）。
    pub fn target_key(self) -> Option<&'static str> {
        match self {
            Product::CodeBuddyIde => Some("software"),
            Product::CodeBuddyIdea => Some("idea"),
            _ => None,
        }
    }

    /// 客户端进程名（Windows 任务管理器里的 exe 全名，**必须带 `.exe`**）。
    ///
    /// CodeBuddy 插件返回空：它的宿主是 JetBrains IDE，由 `providers::codebuddy`
    /// 按本机装了插件的 IDE 现算（见 `JETBRAINS_HOSTS`）。写死 `idea64.exe` 会漏掉
    /// GoLand / PyCharm 这些同样装了插件的宿主。
    pub fn processes(self) -> &'static [&'static str] {
        match self {
            Product::WorkBuddyCn => &["WorkBuddy.exe"],
            Product::WorkBuddyIntl => &["WorkBuddyAI.exe"],
            Product::CodeBuddyIde => &["CodeBuddy CN.exe", "CodeBuddy.exe"],
            Product::CodeBuddyIdea => &[],
        }
    }

    /// 安装位置候选。CodeBuddy 插件没有独立 exe（宿主是 IDE），返回空。
    pub fn installs(self) -> &'static [Install] {
        match self {
            Product::WorkBuddyCn => &[
                Install::Abs(r"C:\App\WB\CN\WorkBuddy.exe"),
                Install::UserPrograms("WorkBuddy", "WorkBuddy.exe"),
                Install::ProgramFiles("WorkBuddy", "WorkBuddy.exe"),
            ],
            Product::WorkBuddyIntl => &[
                Install::Abs(r"C:\App\WB\AI\WorkBuddyAI.exe"),
                Install::UserPrograms("WorkBuddyAI", "WorkBuddyAI.exe"),
                Install::ProgramFiles("WorkBuddyAI", "WorkBuddyAI.exe"),
            ],
            Product::CodeBuddyIde => &[
                Install::UserPrograms("CodeBuddy CN", "CodeBuddy CN.exe"),
                Install::ProgramFiles("CodeBuddy CN", "CodeBuddy CN.exe"),
            ],
            Product::CodeBuddyIdea => &[],
        }
    }

    /// 家目录下的数据目录名。CodeBuddy 系不在家目录。
    pub fn home_dir(self) -> Option<&'static str> {
        match self {
            Product::WorkBuddyCn => Some(".workbuddy"),
            Product::WorkBuddyIntl => Some(".workbuddy-ai"),
            _ => None,
        }
    }

    /// `%APPDATA%` 下的数据目录名。WorkBuddy 系不在 APPDATA。
    pub fn appdata_dir(self) -> Option<&'static str> {
        match self {
            Product::CodeBuddyIde => Some("CodeBuddy CN"),
            Product::CodeBuddyIdea => Some("JetBrains"),
            _ => None,
        }
    }

    /// 登录票据文件名（只在共用的 `CodeBuddyExtension\...\auth` 目录里靠它区分两版）。
    pub fn ticket_file(self) -> Option<&'static str> {
        match self {
            Product::WorkBuddyCn => Some("workbuddy-desktop.info"),
            Product::WorkBuddyIntl => Some("workbuddy-desktop-ai.info"),
            _ => None,
        }
    }

    /// 会话索引里的来源键（`sessions` / `usage` 用）。
    pub fn session_source(self) -> &'static str {
        match self {
            Product::WorkBuddyCn => "cn",
            Product::WorkBuddyIntl => "intl",
            Product::CodeBuddyIde => "CodeBuddyIDE",
            Product::CodeBuddyIdea => "JetBrains",
        }
    }

    pub fn from_account_key(key: &str) -> Option<Product> {
        Product::ALL
            .into_iter()
            .find(|p| p.account_key() == Some(key))
    }

    pub fn from_target_key(key: &str) -> Option<Product> {
        Product::ALL
            .into_iter()
            .find(|p| p.target_key() == Some(key))
    }
}

/// JetBrains 产品目录名 → 宿主进程词干。`IntelliJIdea2026.3` → `idea`。
pub fn host_stem(product_dir: &str) -> Option<&'static str> {
    let name = product_dir.to_lowercase();
    JETBRAINS_HOSTS
        .iter()
        .find(|(prefix, _)| name.starts_with(prefix))
        .map(|(_, stem)| *stem)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **隔离矩阵**：四个产品的身份两两不相交。
    ///
    /// 这条用例就是「不许再搞混」的看门人。串一个名字进来（比如把 `CodeBuddy CN.exe`
    /// 加进 WorkBuddy 的进程表），这里立刻红。
    #[test]
    fn products_never_overlap() {
        // 进程名：同一个 exe 不可能属于两个产品
        for (i, a) in Product::ALL.iter().enumerate() {
            for b in &Product::ALL[i + 1..] {
                for name in a.processes() {
                    assert!(
                        !b.processes().iter().any(|n| n.eq_ignore_ascii_case(name)),
                        "{} 与 {} 共用进程名 {name}",
                        a.label(),
                        b.label()
                    );
                }
            }
        }

        // 显示名：两处叫法不同就是下一次混淆的开始
        for (i, a) in Product::ALL.iter().enumerate() {
            for b in &Product::ALL[i + 1..] {
                assert_ne!(a.label(), b.label(), "两个产品同名");
            }
        }

        // 数据目录：WorkBuddy 两版必须落在不同家目录，CodeBuddy 两个产品不同 APPDATA 目录
        for (i, a) in Product::ALL.iter().enumerate() {
            for b in &Product::ALL[i + 1..] {
                if let (Some(x), Some(y)) = (a.home_dir(), b.home_dir()) {
                    assert_ne!(x, y, "{} 与 {} 共用家目录", a.label(), b.label());
                }
                if let (Some(x), Some(y)) = (a.appdata_dir(), b.appdata_dir()) {
                    assert_ne!(x, y, "{} 与 {} 共用 APPDATA 目录", a.label(), b.label());
                }
            }
        }

        // 票据：两版共用一个 auth 目录，只能靠文件名区分，绝不能相同
        for (i, a) in Product::ALL.iter().enumerate() {
            for b in &Product::ALL[i + 1..] {
                if let (Some(x), Some(y)) = (a.ticket_file(), b.ticket_file()) {
                    assert_ne!(x, y, "{} 与 {} 共用票据文件名", a.label(), b.label());
                }
            }
        }

        // 会话来源键：索引按它分派，撞了就是删错客户的会话
        for (i, a) in Product::ALL.iter().enumerate() {
            for b in &Product::ALL[i + 1..] {
                assert_ne!(a.session_source(), b.session_source(), "会话来源键重复");
            }
        }
    }

    /// 进程名必须是任务管理器里能对上的 exe 全名。
    /// `idea64` 少了 `.exe` 的那次事故，就是这条没被守住。
    #[test]
    fn process_names_are_full_exe_names() {
        for product in Product::ALL {
            for name in product.processes() {
                assert!(
                    name.to_lowercase().ends_with(".exe"),
                    "{} 的进程名 {name} 不是 exe 全名",
                    product.label()
                );
                assert!(!name.contains('\\') && !name.contains('/'), "只写文件名");
            }
        }
    }

    /// 安装位置候选只指向自己：WorkBuddy 的两个版本不能去指 CodeBuddy 的安装位。
    #[test]
    fn install_candidates_belong_to_their_product() {
        for product in Product::ALL {
            for install in product.installs() {
                let text = match install {
                    Install::Abs(p) => p.to_string(),
                    Install::UserPrograms(dir, exe) | Install::ProgramFiles(dir, exe) => {
                        format!("{dir}\\{exe}")
                    }
                };
                let lower = text.to_lowercase();
                let foreign = match product {
                    Product::WorkBuddyCn | Product::WorkBuddyIntl => lower.contains("codebuddy"),
                    Product::CodeBuddyIde => lower.contains("workbuddy"),
                    Product::CodeBuddyIdea => false,
                };
                assert!(
                    !foreign,
                    "{} 的安装位指到了别的产品：{text}",
                    product.label()
                );
            }
        }
    }

    /// 平台键 / 目标键只能被一个产品占用，且能反查回来。
    #[test]
    fn keys_round_trip() {
        for product in Product::ALL {
            if let Some(key) = product.account_key() {
                assert_eq!(Product::from_account_key(key), Some(product));
            }
            if let Some(key) = product.target_key() {
                assert_eq!(Product::from_target_key(key), Some(product));
            }
        }
        // CodeBuddy 不是账号平台：拿它的名字当平台键必须查不到
        assert_eq!(Product::from_account_key("codebuddy"), None);
    }

    /// JetBrains 宿主表：前缀与词干都要能用，且不与其他产品的进程名撞车。
    #[test]
    fn jetbrains_hosts_are_sane() {
        assert_eq!(super::host_stem("IntelliJIdea2026.3"), Some("idea"));
        assert_eq!(super::host_stem("GoLand2026.1"), Some("goland"));
        assert_eq!(super::host_stem("PyCharm2026.1"), Some("pycharm"));
        assert_eq!(super::host_stem("acp-agents"), None);
        for (_, stem) in JETBRAINS_HOSTS {
            for name in [format!("{stem}.exe"), format!("{stem}64.exe")] {
                for product in Product::ALL {
                    assert!(
                        !product
                            .processes()
                            .iter()
                            .any(|n| n.eq_ignore_ascii_case(&name)),
                        "{name} 与 {} 的进程名撞车",
                        product.label()
                    );
                }
            }
        }
    }
}
