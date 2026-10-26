//! The installer window: a dark, fully custom-drawn form with the install folder, shortcut
//! switches, progress and the Install and Cancel buttons. Installation runs on a worker thread
//! that posts updates back.

use crate::install::{self, Shortcuts, Step};
use crate::paint::{self, BACKGROUND, Fonts, HEIGHT, WIDTH, colorref};
use launcher::PRODUCT_NAME;
use std::cell::RefCell;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{CreateFontIndirectW, InvalidateRect, LOGFONTW};
use windows::Win32::Graphics::GdiPlus::{GdiplusStartup, GdiplusStartupInput};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
    VK_ESCAPE, VK_RETURN, VK_SHIFT, VK_SPACE, VK_TAB,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

const WM_UPDATE: u32 = WM_APP + 1;
const WM_FINISHED: u32 = WM_APP + 2;
/// Sent after `TrackMouseEvent` once the pointer leaves the client area.
const WM_MOUSELEAVE: u32 = 0x02a3;
const ANIMATION_TIMER: usize = 1;
/// How long a switch knob takes to cross its track.
const KNOB_SECONDS: f32 = 0.12;

/// The worker's latest status line and progress percentage, read on `WM_UPDATE`.
static UPDATE: Mutex<Option<(String, u32)>> = Mutex::new(None);
/// The worker's result, read on `WM_FINISHED`.
static OUTCOME: Mutex<Option<Result<PathBuf, String>>> = Mutex::new(None);

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

/// A pointer or keyboard target.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Item {
    Toggle(usize),
    Install,
    Cancel,
}

/// Keyboard focus order.
const ORDER: [Item; 5] = [
    Item::Toggle(0),
    Item::Toggle(1),
    Item::Toggle(2),
    Item::Install,
    Item::Cancel,
];

pub(crate) struct Toggle {
    pub(crate) label: &'static str,
    pub(crate) on: bool,
    /// Knob position from 0 (off) to 1 (on), animated toward `on`.
    pub(crate) knob: f32,
}

pub(crate) enum Phase {
    Idle { error: Option<String> },
    Installing { status: String, percent: u32 },
    Done { client: PathBuf },
}

impl Phase {
    pub(crate) fn progress(&self) -> Option<u32> {
        match self {
            Self::Installing { percent, .. } => Some(*percent),
            Self::Done { .. } => Some(100),
            Self::Idle { .. } => None,
        }
    }

    /// The status line and whether it reports an error.
    pub(crate) fn status(&self) -> (&str, bool) {
        match self {
            Self::Idle { error: Some(error) } => (error, true),
            Self::Idle { error: None } => ("", false),
            Self::Installing { status, .. } => (status, false),
            Self::Done { .. } => ("Установка завершена.", false),
        }
    }

    pub(crate) fn button_labels(&self) -> (&'static str, &'static str) {
        match self {
            Self::Done { .. } => ("Играть", "Закрыть"),
            _ => ("Установить", "Отмена"),
        }
    }
}

pub(crate) struct Ui {
    pub(crate) scale: f32,
    pub(crate) icon: HICON,
    pub(crate) fonts: Fonts,
    pub(crate) target: String,
    pub(crate) toggles: [Toggle; 3],
    pub(crate) hover: Option<Item>,
    pub(crate) pressed: Option<Item>,
    pub(crate) focus: Option<Item>,
    /// The focus ring shows only after keyboard navigation.
    pub(crate) keyboard: bool,
    pub(crate) phase: Phase,
    tracking: bool,
    animated: Instant,
}

impl Ui {
    /// Converts 96-DPI pixels to device pixels.
    pub(crate) fn px(&self, value: i32) -> i32 {
        (value as f32 * self.scale).round() as i32
    }

    fn hit(&self, x: i32, y: i32) -> Option<Item> {
        let (x, y) = (
            (x as f32 / self.scale) as i32,
            (y as f32 / self.scale) as i32,
        );
        ORDER.into_iter().find(|item| {
            let rect = paint::bounds(*item);
            x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom
        })
    }

    pub(crate) fn enabled(&self) -> bool {
        !matches!(self.phase, Phase::Installing { .. })
    }
}

/// Opens the window and runs its message loop until it closes.
pub(crate) fn run() {
    // SAFETY: plain Win32 window creation and message pumping on this thread; every handle
    // passed to a call was returned by the system on this thread.
    unsafe {
        let mut token = 0;
        let input = GdiplusStartupInput {
            GdiplusVersion: 1,
            ..Default::default()
        };
        GdiplusStartup(&mut token, &input, std::ptr::null_mut());
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
        let _ = AdjustWindowRectEx(&mut frame, style, false, WINDOW_EX_STYLE(0));
        let (width, height) = (frame.right - frame.left, frame.bottom - frame.top);
        let x = (GetSystemMetrics(SM_CXSCREEN) - width) / 2;
        let y = (GetSystemMetrics(SM_CYSCREEN) - height) / 2;

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
        let font = |height: i32, weight: i32| {
            let mut font: LOGFONTW = metrics.lfMessageFont;
            font.lfHeight = -px(height);
            font.lfWeight = weight;
            CreateFontIndirectW(&font)
        };
        let fonts = Fonts {
            title: font(30, 600),
            body: font(15, 400),
            small: font(13, 400),
            button: font(15, 600),
        };
        UI.with_borrow_mut(|ui| {
            *ui = Some(Ui {
                scale,
                icon,
                fonts,
                target: install::target_dir()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default(),
                toggles: [
                    "Ярлык на рабочем столе",
                    "Ярлык в меню Пуск",
                    "Запустить после установки",
                ]
                .map(|label| Toggle {
                    label,
                    on: true,
                    knob: 1.0,
                }),
                hover: None,
                pressed: None,
                focus: Some(Item::Install),
                keyboard: false,
                phase: Phase::Idle { error: None },
                tracking: false,
                animated: Instant::now(),
            });
        });

        let title = HSTRING::from(format!("Установка {PRODUCT_NAME}"));
        let Ok(window) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
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
        // Dark title bar on Windows 10 20H1+, with the caption matching the form on Windows 11.
        let dark: i32 = 1;
        let caption = colorref(BACKGROUND);
        let _ = DwmSetWindowAttribute(
            window,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&raw const dark).cast(),
            size_of::<i32>() as u32,
        );
        let _ = DwmSetWindowAttribute(
            window,
            DWMWA_CAPTION_COLOR,
            (&raw const caption).cast(),
            size_of_val(&caption) as u32,
        );
        let _ = ShowWindow(window, SW_SHOW);

        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
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
                UI.with_borrow(|ui| {
                    if let Some(ui) = ui {
                        paint::paint(window, ui);
                    }
                });
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_MOUSEMOVE => {
                let (x, y) = point(lparam);
                let track = update(window, |ui| {
                    ui.hover = ui.hit(x, y);
                    !std::mem::replace(&mut ui.tracking, true)
                });
                if track == Some(true) {
                    let mut event = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: window,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut event);
                }
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                update(window, |ui| {
                    ui.tracking = false;
                    ui.hover = None;
                });
                LRESULT(0)
            }
            WM_SETCURSOR if (lparam.0 & 0xffff) as u32 == HTCLIENT => {
                let hand = UI.with_borrow(|ui| {
                    ui.as_ref()
                        .is_some_and(|ui| ui.enabled() && ui.hover.is_some())
                });
                let cursor = if hand { IDC_HAND } else { IDC_ARROW };
                SetCursor(LoadCursorW(None, cursor).ok());
                LRESULT(1)
            }
            WM_LBUTTONDOWN => {
                let (x, y) = point(lparam);
                SetCapture(window);
                update(window, |ui| {
                    ui.pressed = ui.hit(x, y).filter(|_| ui.enabled());
                    ui.keyboard = false;
                });
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let (x, y) = point(lparam);
                let _ = ReleaseCapture();
                let clicked = update(window, |ui| {
                    let pressed = ui.pressed.take();
                    let hit = ui.hit(x, y);
                    let clicked = pressed.filter(|item| Some(*item) == hit);
                    if clicked.is_some() {
                        ui.focus = clicked;
                    }
                    clicked
                })
                .flatten();
                if let Some(item) = clicked {
                    activate(window, item);
                }
                LRESULT(0)
            }
            WM_KEYDOWN => {
                let key = wparam.0 as u16;
                if key == VK_TAB.0 {
                    let back = GetKeyState(VK_SHIFT.0 as i32) < 0;
                    update(window, |ui| {
                        let at = ui
                            .focus
                            .and_then(|focus| ORDER.iter().position(|item| *item == focus));
                        let next = match (at, back) {
                            (None, _) => 0,
                            (Some(at), false) => (at + 1) % ORDER.len(),
                            (Some(at), true) => (at + ORDER.len() - 1) % ORDER.len(),
                        };
                        ui.focus = Some(ORDER[next]);
                        ui.keyboard = true;
                    });
                } else if key == VK_SPACE.0 || key == VK_RETURN.0 {
                    let focus = UI.with_borrow(|ui| ui.as_ref().and_then(|ui| ui.focus));
                    activate(window, focus.unwrap_or(Item::Install));
                } else if key == VK_ESCAPE.0 {
                    activate(window, Item::Cancel);
                }
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == ANIMATION_TIMER => {
                let moving = update(window, |ui| {
                    let now = Instant::now();
                    let step = now.duration_since(ui.animated).as_secs_f32() / KNOB_SECONDS;
                    ui.animated = now;
                    let mut moving = false;
                    for toggle in &mut ui.toggles {
                        let target = if toggle.on { 1.0 } else { 0.0 };
                        toggle.knob += (target - toggle.knob).clamp(-step, step);
                        moving |= toggle.knob != target;
                    }
                    moving
                });
                if moving != Some(true) {
                    let _ = KillTimer(Some(window), ANIMATION_TIMER);
                }
                LRESULT(0)
            }
            WM_UPDATE => {
                let latest = UPDATE.lock().ok().and_then(|mut update| update.take());
                if let Some((status, percent)) = latest {
                    update(window, |ui| {
                        ui.phase = Phase::Installing { status, percent }
                    });
                }
                LRESULT(0)
            }
            WM_FINISHED => {
                finished(window);
                LRESULT(0)
            }
            WM_CLOSE => {
                let busy = UI.with_borrow(|ui| ui.as_ref().is_some_and(|ui| !ui.enabled()));
                if !busy {
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

/// Applies `change` to the window state and repaints.
fn update<T>(window: HWND, change: impl FnOnce(&mut Ui) -> T) -> Option<T> {
    let result = UI.with_borrow_mut(|ui| ui.as_mut().map(change));
    // SAFETY: `window` is this thread's window.
    let _ = unsafe { InvalidateRect(Some(window), None, false) };
    result
}

fn point(lparam: LPARAM) -> (i32, i32) {
    let x = (lparam.0 & 0xffff) as u16 as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xffff) as u16 as i16 as i32;
    (x, y)
}

/// Runs a click or key press on `item`.
fn activate(window: HWND, item: Item) {
    if !UI.with_borrow(|ui| ui.as_ref().is_some_and(Ui::enabled)) {
        return;
    }
    match item {
        Item::Toggle(index) => {
            update(window, |ui| {
                ui.toggles[index].on = !ui.toggles[index].on;
                ui.animated = Instant::now();
            });
            // SAFETY: `window` is this thread's window.
            unsafe { SetTimer(Some(window), ANIMATION_TIMER, 15, None) };
        }
        Item::Cancel => {
            // SAFETY: as above.
            let _ = unsafe { PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        }
        Item::Install => {
            let installed = UI.with_borrow(|ui| match ui.as_ref().map(|ui| &ui.phase) {
                Some(Phase::Done { client }) => Some(client.clone()),
                _ => None,
            });
            match installed {
                Some(client) => launch_and_close(window, &client),
                None => start(window),
            }
        }
    }
}

/// Locks the form and starts the worker with the chosen shortcuts.
fn start(window: HWND) {
    let Some(shortcuts) = update(window, |ui| {
        ui.phase = Phase::Installing {
            status: "Подключение…".to_owned(),
            percent: 0,
        };
        Shortcuts {
            desktop: ui.toggles[0].on,
            start_menu: ui.toggles[1].on,
        }
    }) else {
        return;
    };
    let handle = window.0 as usize;
    std::thread::spawn(move || {
        let window = HWND(handle as *mut c_void);
        let shown = RefCell::new(None);
        let report = |step: Step| {
            let update = describe(step);
            // Only a changed line is posted; the UI keeps the newest one pending.
            if shown.borrow().as_ref() == Some(&update) {
                return;
            }
            *shown.borrow_mut() = Some(update.clone());
            let Ok(mut latest) = UPDATE.lock() else {
                return;
            };
            if latest.replace(update).is_none() {
                // SAFETY: posting to a window owned by another thread is thread-safe.
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

fn describe(step: Step) -> (String, u32) {
    let megabytes = |bytes: u64| bytes / (1 << 20);
    match step {
        Step::Downloading(done, Some(total)) => {
            let percent = (done * 100 / total.max(1)) as u32;
            let (done, total) = (megabytes(done), megabytes(total));
            (
                format!("Скачивание… {percent}% ({done} из {total} МБ)"),
                percent,
            )
        }
        Step::Downloading(done, None) => (format!("Скачивание… {} МБ", megabytes(done)), 0),
        Step::Unpacking => ("Распаковка…".to_owned(), 100),
        Step::Shortcuts => ("Создание ярлыков…".to_owned(), 100),
    }
}

/// Shows the worker's result: launches when asked, or offers to; errors return to the form.
fn finished(window: HWND) {
    let Some(outcome) = OUTCOME.lock().ok().and_then(|mut outcome| outcome.take()) else {
        return;
    };
    let launch = update(window, |ui| {
        let _ = UPDATE.lock().map(|mut update| update.take());
        ui.phase = match &outcome {
            Ok(client) => Phase::Done {
                client: client.clone(),
            },
            Err(error) => Phase::Idle {
                error: Some(format!("Не удалось установить: {error}")),
            },
        };
        ui.toggles[2].on
    });
    if let (Ok(client), Some(true)) = (outcome, launch) {
        launch_and_close(window, &client);
    }
}

/// Starts the installed client and closes the installer; a failed start is shown instead.
fn launch_and_close(window: HWND, client: &Path) {
    let directory = client.parent().unwrap_or(client);
    match std::process::Command::new(client)
        .current_dir(directory)
        .spawn()
    {
        // SAFETY: `window` is this thread's window.
        Ok(_) => unsafe {
            let _ = DestroyWindow(window);
        },
        Err(error) => {
            update(window, |ui| {
                ui.phase = Phase::Idle {
                    error: Some(format!("Не удалось запустить игру: {error}")),
                };
            });
        }
    }
}
