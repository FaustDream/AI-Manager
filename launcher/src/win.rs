//! 状态窗口：托盘左键唤出，点 X 隐藏回托盘（不退出）。
//! 控件：状态行 STATIC + 两个只读 EDIT（运行控制台 / 业务调用链日志）。
//! 窗口回调只做转发，业务在 main.rs 的 App 里（线程局部钩子）。

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, GetSysColorBrush, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, COLOR_WINDOW,
    DEFAULT_CHARSET, FF_MODERN, FF_SWISS, FIXED_PITCH, FW_NORMAL, HFONT, OUT_DEFAULT_PRECIS,
    VARIABLE_PITCH,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{EM_REPLACESEL, EM_SCROLLCARET, EM_SETLIMITTEXT, EM_SETSEL};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, GetWindowTextLengthW, IsIconic,
    LoadCursorW, PostQuitMessage, RegisterClassW, SendMessageW, SetForegroundWindow, SetTimer,
    SetWindowPos, SetWindowTextW, ShowWindow, TranslateMessage, CW_USEDEFAULT, ES_AUTOHSCROLL,
    ES_AUTOVSCROLL, ES_MULTILINE, ES_READONLY, HCURSOR, IDC_ARROW, MSG, SET_WINDOW_POS_FLAGS,
    SWP_NOACTIVATE, SWP_NOZORDER, SW_HIDE, SW_RESTORE, SW_SHOW, WINDOW_EX_STYLE, WINDOW_STYLE,
    WM_CLOSE, WM_DESTROY, WM_ENDSESSION, WM_SETFONT, WM_SIZE, WM_TIMER, WNDCLASSW, WS_CHILD,
    WS_HSCROLL, WS_OVERLAPPEDWINDOW, WS_VISIBLE, WS_VSCROLL,
};

#[derive(Clone, Copy)]
pub struct Widgets {
    pub main: HWND,
    pub status: HWND,
    pub label_console: HWND,
    pub console: HWND,
    pub label_jsonl: HWND,
    pub jsonl: HWND,
}

/// 窗口回调只做转发：tick / close / session_end / layout 的实际逻辑由 main.rs 注入。
pub(crate) struct Hooks {
    pub tick: Option<Box<dyn FnMut()>>,
    pub close: Option<Box<dyn FnMut()>>,
    pub session_end: Option<Box<dyn FnMut()>>,
    pub layout: Option<Box<dyn FnMut(i32, i32)>>,
}

thread_local! {
    static HOOKS: std::cell::RefCell<Hooks> = std::cell::RefCell::new(Hooks {
        tick: None,
        close: None,
        session_end: None,
        layout: None,
    });
}

pub fn set_hooks(hooks: Hooks) {
    HOOKS.with(|slot| *slot.borrow_mut() = hooks);
}

fn run_tick() {
    HOOKS.with(|slot| {
        if let Ok(mut hooks) = slot.try_borrow_mut() {
            if let Some(hook) = hooks.tick.as_mut() {
                hook();
            }
        }
    });
}

fn run_close() {
    HOOKS.with(|slot| {
        if let Ok(mut hooks) = slot.try_borrow_mut() {
            if let Some(hook) = hooks.close.as_mut() {
                hook();
            }
        }
    });
}

fn run_session_end() {
    HOOKS.with(|slot| {
        if let Ok(mut hooks) = slot.try_borrow_mut() {
            if let Some(hook) = hooks.session_end.as_mut() {
                hook();
            }
        }
    });
}

fn run_layout(cx: i32, cy: i32) {
    HOOKS.with(|slot| {
        if let Ok(mut hooks) = slot.try_borrow_mut() {
            if let Some(hook) = hooks.layout.as_mut() {
                hook(cx, cy);
            }
        }
    });
}

pub fn create() -> Result<Widgets, String> {
    unsafe { create_inner() }
}

unsafe fn create_inner() -> Result<Widgets, String> {
    let hinstance = GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW 失败：{e}"))?;
    let class_name: PCWSTR = w!("AIMANAGER_LAUNCHER_WINDOW");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: hinstance.into(),
        hCursor: load_arrow_cursor(),
        hbrBackground: GetSysColorBrush(COLOR_WINDOW),
        lpszClassName: class_name,
        ..Default::default()
    };
    if RegisterClassW(&wc) == 0 {
        return Err("RegisterClassW 失败".to_string());
    }

    let main = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        class_name,
        w!("AI Manager 启动器"),
        WS_OVERLAPPEDWINDOW,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        780,
        540,
        None,
        None,
        None,
        None,
    )
    .map_err(|e| format!("CreateWindowExW 失败：{e}"))?;

    let status = create_child(main, w!("STATIC"), w!("已停止"), static_style())?;
    let label_console = create_child(
        main,
        w!("STATIC"),
        w!("运行控制台（后端输出 · 启动器动作）"),
        static_style(),
    )?;
    let console = create_child(main, w!("EDIT"), w!(""), edit_style())?;
    let label_jsonl = create_child(
        main,
        w!("STATIC"),
        w!("业务调用链日志（data/logs/app.jsonl）"),
        static_style(),
    )?;
    let jsonl = create_child(main, w!("EDIT"), w!(""), edit_style())?;

    // 等宽字体给日志，界面字体给状态行与标签
    let mono: HFONT = CreateFontW(
        -13,
        0,
        0,
        0,
        FW_NORMAL.0 as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET,
        OUT_DEFAULT_PRECIS,
        CLIP_DEFAULT_PRECIS,
        CLEARTYPE_QUALITY,
        FIXED_PITCH.0 as u32 | FF_MODERN.0 as u32,
        w!("Consolas"),
    );
    let ui_font: HFONT = CreateFontW(
        -12,
        0,
        0,
        0,
        FW_NORMAL.0 as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET,
        OUT_DEFAULT_PRECIS,
        CLIP_DEFAULT_PRECIS,
        CLEARTYPE_QUALITY,
        VARIABLE_PITCH.0 as u32 | FF_SWISS.0 as u32,
        w!("Segoe UI"),
    );
    for hwnd in [status, label_console, label_jsonl] {
        SendMessageW(
            hwnd,
            WM_SETFONT,
            Some(WPARAM(ui_font.0 as usize)),
            Some(LPARAM(1)),
        );
    }
    for hwnd in [console, jsonl] {
        SendMessageW(
            hwnd,
            WM_SETFONT,
            Some(WPARAM(mono.0 as usize)),
            Some(LPARAM(1)),
        );
        // 默认上限 32k 字符会静默截断长日志；放宽到 1MB 防极端刷屏吃内存
        SendMessageW(
            hwnd,
            EM_SETLIMITTEXT,
            Some(WPARAM(1_000_000)),
            Some(LPARAM(0)),
        );
    }

    // 状态机心跳：菜单/托盘/编排事件/日志 tail 都在 tick 里消化
    SetTimer(Some(main), 1, 250, None);

    Ok(Widgets {
        main,
        status,
        label_console,
        console,
        label_jsonl,
        jsonl,
    })
}

unsafe fn load_arrow_cursor() -> HCURSOR {
    LoadCursorW(None, IDC_ARROW).unwrap_or_default()
}

unsafe fn create_child(
    parent: HWND,
    class: PCWSTR,
    text: PCWSTR,
    style: WINDOW_STYLE,
) -> Result<HWND, String> {
    CreateWindowExW(
        WINDOW_EX_STYLE(0),
        class,
        text,
        style,
        0,
        0,
        10,
        10,
        Some(parent),
        None,
        None,
        None,
    )
    .map_err(|e| format!("创建控件失败：{e}"))
}

fn static_style() -> WINDOW_STYLE {
    WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0)
}

fn edit_style() -> WINDOW_STYLE {
    WINDOW_STYLE(
        WS_CHILD.0
            | WS_VISIBLE.0
            | WS_VSCROLL.0
            | WS_HSCROLL.0
            | ES_MULTILINE as u32
            | ES_READONLY as u32
            | ES_AUTOVSCROLL as u32
            | ES_AUTOHSCROLL as u32,
    )
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            run_tick();
            LRESULT(0)
        }
        WM_SIZE => {
            let cx = (lparam.0 & 0xFFFF) as u16 as i32;
            let cy = ((lparam.0 >> 16) & 0xFFFF) as u16 as i32;
            run_layout(cx, cy);
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        // 点 X = 隐藏回托盘，不销毁窗口、不退出进程
        WM_CLOSE => {
            run_close();
            LRESULT(0)
        }
        // 注销/关机：按既定策略停掉自己拉起的服务（尽力而为）
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                run_session_end();
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// 追加文本到只读 EDIT：caret 移到末尾再替换，不整窗重置（保持滚动位置语义）。
pub unsafe fn append(hwnd: HWND, text: &str) {
    let len = GetWindowTextLengthW(hwnd);
    SendMessageW(
        hwnd,
        EM_SETSEL,
        Some(WPARAM(len as usize)),
        Some(LPARAM(len as isize)),
    );
    let wide = HSTRING::from(text);
    SendMessageW(
        hwnd,
        EM_REPLACESEL,
        Some(WPARAM(0)),
        Some(LPARAM(wide.as_ptr() as isize)),
    );
    SendMessageW(hwnd, EM_SCROLLCARET, Some(WPARAM(0)), Some(LPARAM(0)));
}

pub unsafe fn set_text(hwnd: HWND, text: &str) {
    let _ = SetWindowTextW(hwnd, &HSTRING::from(text));
}

/// 追加一行：非空窗格自动补换行，保持每行一条。
pub unsafe fn append_line(hwnd: HWND, line: &str) {
    let prefix = if GetWindowTextLengthW(hwnd) == 0 {
        ""
    } else {
        "\n"
    };
    append(hwnd, &format!("{prefix}{line}"));
}

pub unsafe fn show(widgets: &Widgets) {
    let _ = ShowWindow(widgets.main, SW_SHOW);
    if IsIconic(widgets.main).as_bool() {
        let _ = ShowWindow(widgets.main, SW_RESTORE);
    }
    let _ = SetForegroundWindow(widgets.main);
}

pub unsafe fn hide(widgets: &Widgets) {
    let _ = ShowWindow(widgets.main, SW_HIDE);
}

/// 布局：状态行 + 两个标签 + 两个日志窗格，等分剩余高度。
pub fn layout(widgets: &Widgets, cx: i32, cy: i32) {
    unsafe {
        let m = 12;
        let status_h = 26;
        let label_h = 20;
        let gap = 6;
        let width = (cx - 2 * m).max(120);

        let y_status = m;
        let y_label1 = y_status + status_h + gap;
        let y_pane1 = y_label1 + label_h + gap;
        let pane_h = ((cy - y_pane1 - gap - label_h - gap - m) / 2).max(80);
        let y_label2 = y_pane1 + pane_h + gap;
        let y_pane2 = y_label2 + label_h + gap;

        let _ = place(widgets.status, m, y_status, width, status_h);
        let _ = place(widgets.label_console, m, y_label1, width, label_h);
        let _ = place(widgets.console, m, y_pane1, width, pane_h);
        let _ = place(widgets.label_jsonl, m, y_label2, width, label_h);
        let _ = place(widgets.jsonl, m, y_pane2, width, cy - y_pane2 - m);
    }
}

unsafe fn place(hwnd: HWND, x: i32, y: i32, w: i32, h: i32) -> windows::core::Result<()> {
    SetWindowPos(
        hwnd,
        None,
        x,
        y,
        w,
        h,
        SET_WINDOW_POS_FLAGS(SWP_NOZORDER.0 | SWP_NOACTIVATE.0),
    )
}

/// 消息泵：GetMessage 返回 0（收到 WM_QUIT）即结束。
pub fn pump_until_quit() {
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
