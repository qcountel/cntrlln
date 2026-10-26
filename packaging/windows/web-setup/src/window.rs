//! The installer window: a branded header, the install folder, shortcut choices, progress and
//! the Install and Cancel buttons. Installation runs on a worker thread that posts updates back.

use crate::install::{self, Shortcuts, Step};
use launcher::PRODUCT_NAME;
use std::cell::RefCell;
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Mutex;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontIndirectW, CreateSolidBrush, DT_LEFT, DT_SINGLELINE, DT_TOP, DrawTextW,
    EndPaint, FillRect, GetStockObject, HBRUSH, HDC, HFONT, PAINTSTRUCT, SelectObject, SetBkColor,
    SetBkMode, SetTextColor, TRANSPARENT, WHITE_BRUSH,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    BST_CHECKED, ICC_PROGRESS_CLASS, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX,
    InitCommonControlsEx, PBM_SETPOS, PBM_SETRANGE32,
};
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

const INSTALL: i32 = 1;
const CANCEL: i32 = 2;
const DESKTOP: i32 = 10;
const START_MENU: i32 = 11;
const LAUNCH: i32 = 12;
const WM_UPDATE: u32 = WM_APP + 1;
const WM_FINISHED: u32 = WM_APP + 2;
/// Static control styles that shorten long text with an ellipsis.
const PATH_ELLIPSIS: u32 = 0x8000;
const END_ELLIPSIS: u32 = 0x4000;

/// Client size and header height in 96-DPI pixels.
const WIDTH: i32 = 540;
const HEIGHT: i32 = 360;
const HEADER: i32 = 96;
const MARGIN: i32 = 24;
const HEADER_COLOR: COLORREF = rgb(0x17, 0x17, 0x1f);
const SUBTITLE_COLOR: COLORREF = rgb(0xb4, 0xb4, 0xc3);
const MUTED_COLOR: COLORREF = rgb(0x5f, 0x5f, 0x6b);

/// The worker's latest status line and progress percentage, read on `WM_UPDATE`.
static UPDATE: Mutex<Option<(String, u32)>> = Mutex::new(None);
/// The worker's result, read on `WM_FINISHED`.
static OUTCOME: Mutex<Option<Result<PathBuf, String>>> = Mutex::new(None);

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

struct Ui {
    scale: f32,
    icon: HICON,
    title_font: HFONT,
    body_font: HFONT,
    header_brush: HBRUSH,
    install: HWND,
    cancel: HWND,
    checkboxes: [HWND; 3],
    progress: HWND,
    status: HWND,
    installing: bool,
    installed: Option<PathBuf>,
}

const fn rgb(red: u8, green: u8, blue: u8) -> COLORREF {
    COLORREF(red as u32 | (green as u32) << 8 | (blue as u32) << 16)
}

/// Opens the window and runs its message loop until it closes.
pub(crate) fn run() {
    // SAFETY: plain Win32 window creation and message pumping on this thread; every handle
    // passed to a call was returned by the system on this thread.
    unsafe {
        let _ = InitCommonControlsEx(&INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_PROGRESS_CLASS | ICC_STANDARD_CLASSES,
        });
        let instance: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        let scale = GetDpiForSystem() as f32 / 96.0;
        let px = |value: i32| (value as f32 * scale).round() as i32;
        let icon = HICON(
            LoadImageW(
                Some(instance),
                PCWSTR(1 as _),
                IMAGE_ICON,
                px(56),
                px(56),
                LR_DEFAULTCOLOR,
            )
            .unwrap_or_default()
            .0,
        );
        let class = w!("ControllinSetup");
        RegisterClassExW(&WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(procedure),
            hInstance: instance,
            hIcon: LoadIconW(Some(instance), PCWSTR(1 as _)).unwrap_or_default(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH(GetStockObject(WHITE_BRUSH).0),
            lpszClassName: class,
            ..Default::default()
        });
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let mut frame = RECT {
            left: 0,
            top: 0,
            right: px(WIDTH),
            bottom: px(HEIGHT),
        };
        let _ = AdjustWindowRectEx(&mut frame, style, false, WS_EX_CONTROLPARENT);
        let (width, height) = (frame.right - frame.left, frame.bottom - frame.top);
        let x = (GetSystemMetrics(SM_CXSCREEN) - width) / 2;
        let y = (GetSystemMetrics(SM_CYSCREEN) - height) / 2;
        let title = HSTRING::from(format!("Установка {PRODUCT_NAME}"));
        let Ok(window) = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            class,
            &title,
            style,
            x,
            y,
            width,
            height,
            None,
            None,
            Some(instance),
            None,
        ) else {
            return;
        };

        let mut metrics = NONCLIENTMETRICSW {
            cbSize: size_of::<NONCLIENTMETRICSW>() as u32,
            ..Default::default()
        };
        let _ = SystemParametersInfoW(
            SPI_GETNONCLIENTMETRICS,
            metrics.cbSize,
            Some((&raw mut metrics).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        let mut body = metrics.lfMessageFont;
        body.lfHeight = -px(15);
        let mut heading = body;
        heading.lfHeight = -px(30);
        heading.lfWeight = 600;

        let control = |class: PCWSTR, text: &str, style: u32, id: i32, rect: [i32; 4]| {
            let child = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                &HSTRING::from(text),
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(style),
                px(rect[0]),
                px(rect[1]),
                px(rect[2]),
                px(rect[3]),
                Some(window),
                Some(HMENU(id as usize as *mut c_void)),
                Some(instance),
                None,
            )
            .unwrap_or_default();
            child
        };
        let checkbox = (WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32) as u32;
        let target = install::target_dir()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        let content = WIDTH - 2 * MARGIN;
        let label = control(
            w!("STATIC"),
            "Папка установки",
            0,
            0,
            [MARGIN, 116, content, 20],
        );
        let path = control(
            w!("STATIC"),
            &target,
            PATH_ELLIPSIS,
            0,
            [MARGIN, 138, content, 20],
        );
        let checkboxes = [
            control(
                w!("BUTTON"),
                "Ярлык на рабочем столе",
                checkbox,
                DESKTOP,
                [MARGIN, 172, 320, 24],
            ),
            control(
                w!("BUTTON"),
                "Ярлык в меню Пуск",
                checkbox,
                START_MENU,
                [MARGIN, 198, 320, 24],
            ),
            control(
                w!("BUTTON"),
                "Запустить после установки",
                checkbox,
                LAUNCH,
                [MARGIN, 224, 320, 24],
            ),
        ];
        let progress = control(
            w!("msctls_progress32"),
            "",
            0,
            0,
            [MARGIN, 262, content, 16],
        );
        let status = control(
            w!("STATIC"),
            "",
            END_ELLIPSIS,
            0,
            [MARGIN, 284, content, 20],
        );
        let install = control(
            w!("BUTTON"),
            "Установить",
            (WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32) as u32,
            INSTALL,
            [WIDTH - MARGIN - 96 - 8 - 112, 312, 112, 32],
        );
        let cancel = control(
            w!("BUTTON"),
            "Отмена",
            (WS_TABSTOP.0 | BS_PUSHBUTTON as u32) as u32,
            CANCEL,
            [WIDTH - MARGIN - 96, 312, 96, 32],
        );
        let _ = ShowWindow(progress, SW_HIDE);
        SendMessageW(progress, PBM_SETRANGE32, Some(WPARAM(0)), Some(LPARAM(100)));

        let body_font = CreateFontIndirectW(&body);
        let title_font = CreateFontIndirectW(&heading);
        for child in [label, path, status, install, cancel, progress]
            .into_iter()
            .chain(checkboxes)
        {
            SendMessageW(
                child,
                WM_SETFONT,
                Some(WPARAM(body_font.0 as usize)),
                Some(LPARAM(1)),
            );
        }
        for checkbox in checkboxes {
            SendMessageW(
                checkbox,
                BM_SETCHECK,
                Some(WPARAM(BST_CHECKED.0 as usize)),
                None,
            );
        }
        UI.with_borrow_mut(|ui| {
            *ui = Some(Ui {
                scale,
                icon,
                title_font,
                body_font,
                header_brush: CreateSolidBrush(HEADER_COLOR),
                install,
                cancel,
                checkboxes,
                progress,
                status,
                installing: false,
                installed: None,
            });
        });
        let _ = ShowWindow(window, SW_SHOW);
        let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(install));

        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            if IsDialogMessageW(window, &message).as_bool() {
                continue;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

extern "system" fn procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: called by the system on the window's thread with that window's messages.
    unsafe {
        match message {
            WM_PAINT => {
                paint(window);
                LRESULT(0)
            }
            WM_CTLCOLORSTATIC => {
                let hdc = HDC(wparam.0 as *mut c_void);
                let muted = UI.with_borrow(|ui| {
                    ui.as_ref()
                        .is_some_and(|ui| ui.status.0 == lparam.0 as *mut c_void)
                });
                SetBkColor(hdc, rgb(0xff, 0xff, 0xff));
                if muted {
                    SetTextColor(hdc, MUTED_COLOR);
                }
                LRESULT(GetStockObject(WHITE_BRUSH).0 as isize)
            }
            WM_COMMAND => {
                match (wparam.0 & 0xffff) as i32 {
                    INSTALL => on_install(window),
                    CANCEL => {
                        let _ = PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0));
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_UPDATE => {
                if let Some((text, percent)) =
                    UPDATE.lock().ok().and_then(|mut update| update.take())
                {
                    UI.with_borrow(|ui| {
                        if let Some(ui) = ui {
                            let _ = SetWindowTextW(ui.status, &HSTRING::from(text));
                            SendMessageW(
                                ui.progress,
                                PBM_SETPOS,
                                Some(WPARAM(percent as usize)),
                                None,
                            );
                        }
                    });
                }
                LRESULT(0)
            }
            WM_FINISHED => {
                finished(window);
                LRESULT(0)
            }
            WM_CLOSE => {
                if !UI.with_borrow(|ui| ui.as_ref().is_some_and(|ui| ui.installing)) {
                    let _ = DestroyWindow(window);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(window, message, wparam, lparam),
        }
    }
}

/// Draws the dark header with the icon, product name and tagline.
unsafe fn paint(window: HWND) {
    // SAFETY: paints between BeginPaint and EndPaint with objects owned by `UI`.
    unsafe {
        let mut paint = PAINTSTRUCT::default();
        let hdc = BeginPaint(window, &mut paint);
        UI.with_borrow(|ui| {
            let Some(ui) = ui else { return };
            let px = |value: i32| (value as f32 * ui.scale).round() as i32;
            let header = RECT {
                left: 0,
                top: 0,
                right: px(WIDTH),
                bottom: px(HEADER),
            };
            FillRect(hdc, &header, ui.header_brush);
            let _ = DrawIconEx(
                hdc,
                px(MARGIN),
                px(20),
                ui.icon,
                px(56),
                px(56),
                0,
                None,
                DI_NORMAL,
            );
            SetBkMode(hdc, TRANSPARENT);
            let text = |font: HFONT, color: COLORREF, top: i32, content: &str| {
                SelectObject(hdc, font.into());
                SetTextColor(hdc, color);
                let mut wide: Vec<u16> = content.encode_utf16().collect();
                let mut rect = RECT {
                    left: px(MARGIN + 56 + 16),
                    top: px(top),
                    right: px(WIDTH - MARGIN),
                    bottom: px(HEADER),
                };
                DrawTextW(hdc, &mut wide, &mut rect, DT_LEFT | DT_TOP | DT_SINGLELINE);
            };
            text(ui.title_font, rgb(0xff, 0xff, 0xff), 18, PRODUCT_NAME);
            text(
                ui.body_font,
                SUBTITLE_COLOR,
                58,
                "Клиент Minecraft Bedrock с полной поддержкой геймпада",
            );
        });
        let _ = EndPaint(window, &paint);
    }
}

/// Locks the form and starts the worker with the chosen shortcuts.
unsafe fn on_install(window: HWND) {
    // SAFETY: control handles in `UI` belong to this window's thread.
    let shortcuts = unsafe {
        UI.with_borrow_mut(|ui| {
            let ui = ui.as_mut()?;
            if let Some(client) = ui.installed.clone() {
                launch_and_close(window, &client, false);
                return None;
            }
            ui.installing = true;
            for control in [ui.install, ui.cancel].into_iter().chain(ui.checkboxes) {
                let _ = EnableWindow(control, false);
            }
            let _ = ShowWindow(ui.progress, SW_SHOW);
            let checked = |control: HWND| {
                SendMessageW(control, BM_GETCHECK, None, None).0 as u32 == BST_CHECKED.0
            };
            Some(Shortcuts {
                desktop: checked(ui.checkboxes[0]),
                start_menu: checked(ui.checkboxes[1]),
            })
        })
    };
    let Some(shortcuts) = shortcuts else { return };
    let handle = window.0 as usize;
    std::thread::spawn(move || {
        let window = HWND(handle as *mut c_void);
        let shown = std::cell::RefCell::new(None);
        let report = |step: Step| {
            let update = match step {
                Step::Downloading(done, total) => {
                    let megabytes = |bytes: u64| bytes / (1 << 20);
                    match total {
                        Some(total) => {
                            let percent = (done * 100 / total.max(1)) as u32;
                            (
                                format!(
                                    "Скачивание… {percent}% ({} из {} МБ)",
                                    megabytes(done),
                                    megabytes(total)
                                ),
                                percent,
                            )
                        }
                        None => (format!("Скачивание… {} МБ", megabytes(done)), 0),
                    }
                }
                Step::Unpacking => ("Распаковка…".to_owned(), 100),
                Step::Shortcuts => ("Создание ярлыков…".to_owned(), 100),
            };
            // Only a changed line is posted; the UI keeps the newest one pending.
            if shown.borrow().as_ref() == Some(&update) {
                return;
            }
            *shown.borrow_mut() = Some(update.clone());
            let Ok(mut latest) = UPDATE.lock() else {
                return;
            };
            let pending = latest.replace(update).is_some();
            if !pending {
                // SAFETY: posting to a window handle owned by the UI thread is thread-safe.
                let _ = unsafe { PostMessageW(Some(window), WM_UPDATE, WPARAM(0), LPARAM(0)) };
            }
        };
        let outcome = install::install(shortcuts, &report).map_err(|error| error.to_string());
        if let Ok(mut slot) = OUTCOME.lock() {
            *slot = Some(outcome);
        }
        // SAFETY: as above.
        let _ = unsafe { PostMessageW(Some(window), WM_FINISHED, WPARAM(0), LPARAM(0)) };
    });
}

/// Shows the worker's result: launches or offers to, or reports the error and unlocks the form.
unsafe fn finished(window: HWND) {
    let Some(outcome) = OUTCOME.lock().ok().and_then(|mut outcome| outcome.take()) else {
        return;
    };
    // SAFETY: control handles in `UI` belong to this window's thread.
    unsafe {
        let launch = UI.with_borrow_mut(|ui| {
            let ui = ui.as_mut()?;
            ui.installing = false;
            let launch =
                SendMessageW(ui.checkboxes[2], BM_GETCHECK, None, None).0 as u32 == BST_CHECKED.0;
            match &outcome {
                Ok(client) => {
                    ui.installed = Some(client.clone());
                    let _ = SetWindowTextW(ui.status, &HSTRING::from("Установка завершена."));
                    let _ = SetWindowTextW(ui.install, w!("Играть"));
                    let _ = SetWindowTextW(ui.cancel, w!("Закрыть"));
                    let _ = EnableWindow(ui.install, true);
                    let _ = EnableWindow(ui.cancel, true);
                }
                Err(_) => {
                    let _ = ShowWindow(ui.progress, SW_HIDE);
                    let _ = SetWindowTextW(ui.status, &HSTRING::new());
                    for control in [ui.install, ui.cancel].into_iter().chain(ui.checkboxes) {
                        let _ = EnableWindow(control, true);
                    }
                }
            }
            Some(launch)
        });
        match outcome {
            Ok(client) if launch == Some(true) => launch_and_close(window, &client, true),
            Ok(_) => {}
            Err(error) => {
                MessageBoxW(
                    Some(window),
                    &HSTRING::from(format!("Не удалось установить {PRODUCT_NAME}.\n\n{error}")),
                    &HSTRING::from(format!("Установка {PRODUCT_NAME}")),
                    MB_OK | MB_ICONERROR,
                );
            }
        }
    }
}

/// Starts the installed client and closes the installer; a failed start is reported instead.
unsafe fn launch_and_close(window: HWND, client: &std::path::Path, after_install: bool) {
    let directory = client.parent().unwrap_or(client);
    match std::process::Command::new(client)
        .current_dir(directory)
        .spawn()
    {
        // SAFETY: `window` is this thread's window.
        Ok(_) => unsafe {
            let _ = DestroyWindow(window);
        },
        Err(error) => unsafe {
            let prefix = if after_install {
                "Установка завершена, но игру"
            } else {
                "Игру"
            };
            MessageBoxW(
                Some(window),
                &HSTRING::from(format!("{prefix} не удалось запустить: {error}")),
                &HSTRING::from(PRODUCT_NAME),
                MB_OK | MB_ICONWARNING,
            );
        },
    }
}
