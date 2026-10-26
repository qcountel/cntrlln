//! Layout and drawing of the dark installer window: anti-aliased shapes through GDI+ and text
//! through GDI, into an off-screen buffer that is copied to the window in one step.

use crate::window::{Item, Phase, Ui};
use launcher::PRODUCT_NAME;
use std::ptr::null_mut;
use windows::Win32::Foundation::{COLORREF, HWND, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::GdiPlus::*;
use windows::Win32::UI::WindowsAndMessaging::{DI_NORMAL, DrawIconEx};

/// Client size and regions in 96-DPI pixels.
pub(crate) const WIDTH: i32 = 540;
pub(crate) const HEIGHT: i32 = 392;
const HEADER: i32 = 96;
const MARGIN: i32 = 24;
const TOGGLE_TOP: i32 = 184;
const TOGGLE_PITCH: i32 = 36;
const ROW: i32 = 28;
const SWITCH: (f32, f32) = (44.0, 24.0);
const BUTTONS_TOP: i32 = 332;
const BUTTON_HEIGHT: i32 = 36;
const INSTALL_WIDTH: i32 = 136;
const CANCEL_WIDTH: i32 = 104;
const BUTTON_GAP: i32 = 10;

/// Palette as `0xRRGGBB`.
pub(crate) const BACKGROUND: u32 = 0x15151c;
const HEADER_FILL: u32 = 0x0e0e13;
const DIVIDER: u32 = 0x262631;
const FIELD: u32 = 0x1e1e27;
const TEXT: u32 = 0xececf1;
const MUTED: u32 = 0x9a9aa8;
const ACCENT: u32 = 0xe0483a;
const ACCENT_HOVER: u32 = 0xec5a4c;
const ACCENT_PRESSED: u32 = 0xc63d31;
const SECONDARY: u32 = 0x272732;
const SECONDARY_HOVER: u32 = 0x31313d;
const TRACK_OFF: u32 = 0x3a3a46;
const ERROR: u32 = 0xff6b5e;

/// Fonts the window creates once for the system DPI.
pub(crate) struct Fonts {
    pub(crate) title: HFONT,
    pub(crate) body: HFONT,
    pub(crate) small: HFONT,
    pub(crate) button: HFONT,
}

/// The clickable region of `item` in 96-DPI pixels.
pub(crate) fn bounds(item: Item) -> RECT {
    match item {
        Item::Toggle(index) => {
            let top = TOGGLE_TOP + TOGGLE_PITCH * index as i32;
            RECT {
                left: MARGIN,
                top,
                right: WIDTH - MARGIN,
                bottom: top + ROW,
            }
        }
        Item::Cancel => RECT {
            left: WIDTH - MARGIN - CANCEL_WIDTH,
            top: BUTTONS_TOP,
            right: WIDTH - MARGIN,
            bottom: BUTTONS_TOP + BUTTON_HEIGHT,
        },
        Item::Install => {
            let right = WIDTH - MARGIN - CANCEL_WIDTH - BUTTON_GAP;
            RECT {
                left: right - INSTALL_WIDTH,
                top: BUTTONS_TOP,
                right,
                bottom: BUTTONS_TOP + BUTTON_HEIGHT,
            }
        }
    }
}

/// Paints the whole client area of `window` from `ui`.
///
/// # Safety
/// Must run on the window's thread while handling `WM_PAINT`.
pub(crate) unsafe fn paint(window: HWND, ui: &Ui) {
    // SAFETY: every GDI and GDI+ object created here is released before returning, and the
    // buffer DC outlives the GDI+ graphics drawn into it.
    unsafe {
        let mut paint = PAINTSTRUCT::default();
        let target = BeginPaint(window, &mut paint);
        let (width, height) = (ui.px(WIDTH), ui.px(HEIGHT));
        let buffer = CreateCompatibleDC(Some(target));
        let bitmap = CreateCompatibleBitmap(target, width, height);
        let previous = SelectObject(buffer, bitmap.into());

        let mut graphics = null_mut();
        GdipCreateFromHDC(buffer, &mut graphics);
        GdipSetSmoothingMode(graphics, SmoothingModeAntiAlias);
        GdipSetPixelOffsetMode(graphics, PixelOffsetModeHalf);
        let shapes = Shapes {
            graphics,
            scale: ui.scale,
        };
        shapes.rect(0.0, 0.0, WIDTH as f32, HEIGHT as f32, 0.0, BACKGROUND);
        shapes.rect(0.0, 0.0, WIDTH as f32, HEADER as f32, 0.0, HEADER_FILL);
        shapes.rect(0.0, HEADER as f32, WIDTH as f32, 1.0, 0.0, DIVIDER);
        shapes.rect(
            MARGIN as f32,
            136.0,
            (WIDTH - 2 * MARGIN) as f32,
            34.0,
            8.0,
            FIELD,
        );
        let enabled = !matches!(ui.phase, Phase::Installing { .. });
        for (index, toggle) in ui.toggles.iter().enumerate() {
            let row = bounds(Item::Toggle(index));
            let x = row.right as f32 - SWITCH.0;
            let y = row.top as f32 + (ROW as f32 - SWITCH.1) / 2.0;
            let hovered = enabled && ui.hover == Some(Item::Toggle(index));
            let off = if hovered { 0x464653 } else { TRACK_OFF };
            let track = dim(mix(off, ACCENT, toggle.knob), enabled);
            shapes.rect(x, y, SWITCH.0, SWITCH.1, SWITCH.1 / 2.0, track);
            let knob = SWITCH.1 - 6.0;
            let travel = SWITCH.0 - SWITCH.1;
            shapes.circle(
                x + 3.0 + travel * toggle.knob,
                y + 3.0,
                knob,
                dim(0xffffff, enabled),
            );
        }
        if let Some(percent) = ui.phase.progress() {
            let width = (WIDTH - 2 * MARGIN) as f32;
            shapes.rect(MARGIN as f32, 296.0, width, 6.0, 3.0, SECONDARY);
            let filled = width * percent.min(100) as f32 / 100.0;
            if filled > 0.0 {
                shapes.rect(MARGIN as f32, 296.0, filled.max(6.0), 6.0, 3.0, ACCENT);
            }
        }
        for (item, primary) in [(Item::Install, true), (Item::Cancel, false)] {
            let rect = bounds(item);
            let state = (ui.hover == Some(item), ui.pressed == Some(item));
            let fill = match (primary, state) {
                (true, (_, true)) => ACCENT_PRESSED,
                (true, (true, _)) => ACCENT_HOVER,
                (true, _) => ACCENT,
                (false, (true, _)) => SECONDARY_HOVER,
                (false, _) => SECONDARY,
            };
            let (x, y) = (rect.left as f32, rect.top as f32);
            let (w, h) = (
                (rect.right - rect.left) as f32,
                (rect.bottom - rect.top) as f32,
            );
            shapes.rect(x, y, w, h, 8.0, dim(fill, enabled));
        }
        if let Some(focus) = ui.focus.filter(|_| ui.keyboard) {
            let rect = bounds(focus);
            shapes.outline(
                rect,
                if matches!(focus, Item::Toggle(_)) {
                    6.0
                } else {
                    10.0
                },
            );
        }
        GdipDeleteGraphics(graphics);

        let _ = DrawIconEx(
            buffer,
            ui.px(MARGIN),
            ui.px(20),
            ui.icon,
            ui.px(56),
            ui.px(56),
            0,
            None,
            DI_NORMAL,
        );
        SetBkMode(buffer, TRANSPARENT);
        let text = Text { dc: buffer, ui };
        let header_text = MARGIN + 56 + 16;
        text.draw(
            ui.fonts.title,
            TEXT,
            [header_text, 16, WIDTH - MARGIN, 56],
            PRODUCT_NAME,
            DT_LEFT,
        );
        text.draw(
            ui.fonts.body,
            MUTED,
            [header_text, 56, WIDTH - MARGIN, 80],
            "Клиент Minecraft Bedrock с полной поддержкой геймпада",
            DT_LEFT | DT_END_ELLIPSIS,
        );
        text.draw(
            ui.fonts.small,
            MUTED,
            [MARGIN, 112, WIDTH - MARGIN, 132],
            "Папка установки",
            DT_LEFT,
        );
        text.draw(
            ui.fonts.body,
            TEXT,
            [MARGIN + 12, 136, WIDTH - MARGIN - 12, 170],
            &ui.target,
            DT_LEFT | DT_PATH_ELLIPSIS,
        );
        for (index, toggle) in ui.toggles.iter().enumerate() {
            let row = bounds(Item::Toggle(index));
            let rect = [
                row.left,
                row.top,
                row.right - SWITCH.0 as i32 - 12,
                row.bottom,
            ];
            text.draw(
                ui.fonts.body,
                dim(TEXT, enabled),
                rect,
                toggle.label,
                DT_LEFT,
            );
        }
        let (status, color) = ui.phase.status();
        text.draw(
            ui.fonts.small,
            if color { ERROR } else { MUTED },
            [MARGIN, 304, WIDTH - MARGIN, 326],
            status,
            DT_LEFT | DT_END_ELLIPSIS,
        );
        let (install, cancel) = ui.phase.button_labels();
        for (item, label, color) in [
            (Item::Install, install, 0xffffff),
            (Item::Cancel, cancel, TEXT),
        ] {
            let rect = bounds(item);
            let rect = [rect.left, rect.top, rect.right, rect.bottom];
            text.draw(ui.fonts.button, dim(color, enabled), rect, label, DT_CENTER);
        }

        let _ = BitBlt(target, 0, 0, width, height, Some(buffer), 0, 0, SRCCOPY);
        SelectObject(buffer, previous);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(buffer);
        let _ = EndPaint(window, &paint);
    }
}

/// Anti-aliased fills in 96-DPI pixels.
struct Shapes {
    graphics: *mut GpGraphics,
    scale: f32,
}

impl Shapes {
    /// A rectangle with corner `radius`; zero draws square corners.
    unsafe fn rect(&self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: u32) {
        // SAFETY: the path and brush are created and released around one fill.
        unsafe {
            let path = self.path(x, y, w, h, radius);
            let mut brush = null_mut();
            GdipCreateSolidFill(argb(color), &mut brush);
            GdipFillPath(self.graphics, brush.cast(), path);
            GdipDeleteBrush(brush.cast());
            GdipDeletePath(path);
        }
    }

    unsafe fn circle(&self, x: f32, y: f32, diameter: f32, color: u32) {
        let s = self.scale;
        // SAFETY: the brush is created and released around one fill.
        unsafe {
            let mut brush = null_mut();
            GdipCreateSolidFill(argb(color), &mut brush);
            GdipFillEllipse(
                self.graphics,
                brush.cast(),
                x * s,
                y * s,
                diameter * s,
                diameter * s,
            );
            GdipDeleteBrush(brush.cast());
        }
    }

    /// The keyboard focus ring around `rect`.
    unsafe fn outline(&self, rect: RECT, radius: f32) {
        // SAFETY: the path and pen are created and released around one stroke.
        unsafe {
            let inset = -3.0;
            let path = self.path(
                rect.left as f32 + inset,
                rect.top as f32 + inset,
                (rect.right - rect.left) as f32 - 2.0 * inset,
                (rect.bottom - rect.top) as f32 - 2.0 * inset,
                radius,
            );
            let mut pen = null_mut();
            GdipCreatePen1(argb(ACCENT_HOVER), 2.0 * self.scale, UnitPixel, &mut pen);
            GdipDrawPath(self.graphics, pen, path);
            GdipDeletePen(pen);
            GdipDeletePath(path);
        }
    }

    unsafe fn path(&self, x: f32, y: f32, w: f32, h: f32, radius: f32) -> *mut GpPath {
        let s = self.scale;
        let (x, y, w, h) = (x * s, y * s, w * s, h * s);
        let d = (radius * s * 2.0).min(w).min(h);
        // SAFETY: the caller releases the returned path.
        unsafe {
            let mut path = null_mut();
            GdipCreatePath(FillModeAlternate, &mut path);
            if d <= 0.0 {
                GdipAddPathRectangle(path, x, y, w, h);
            } else {
                GdipAddPathArc(path, x, y, d, d, 180.0, 90.0);
                GdipAddPathArc(path, x + w - d, y, d, d, 270.0, 90.0);
                GdipAddPathArc(path, x + w - d, y + h - d, d, d, 0.0, 90.0);
                GdipAddPathArc(path, x, y + h - d, d, d, 90.0, 90.0);
                GdipClosePathFigure(path);
            }
            path
        }
    }
}

/// GDI text in 96-DPI pixel boxes, vertically centered on one line.
struct Text<'a> {
    dc: HDC,
    ui: &'a Ui,
}

impl Text<'_> {
    unsafe fn draw(
        &self,
        font: HFONT,
        color: u32,
        rect: [i32; 4],
        text: &str,
        flags: DRAW_TEXT_FORMAT,
    ) {
        // An empty slice would hand DrawTextW a dangling pointer.
        if text.is_empty() {
            return;
        }
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        let mut rect = RECT {
            left: self.ui.px(rect[0]),
            top: self.ui.px(rect[1]),
            right: self.ui.px(rect[2]),
            bottom: self.ui.px(rect[3]),
        };
        // SAFETY: draws into this paint's buffer DC with a font the window owns.
        unsafe {
            SelectObject(self.dc, font.into());
            SetTextColor(self.dc, colorref(color));
            DrawTextW(
                self.dc,
                &mut wide,
                &mut rect,
                flags | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
            );
        }
    }
}

/// Linear blend from `from` to `to` by `amount` in 0..=1.
fn mix(from: u32, to: u32, amount: f32) -> u32 {
    let channel = |shift: u32| {
        let (a, b) = ((from >> shift) & 0xff, (to >> shift) & 0xff);
        ((a as f32 + (b as f32 - a as f32) * amount.clamp(0.0, 1.0)).round() as u32) << shift
    };
    channel(16) | channel(8) | channel(0)
}

/// Fades a disabled control's color toward the background.
fn dim(color: u32, enabled: bool) -> u32 {
    if enabled {
        color
    } else {
        mix(color, BACKGROUND, 0.55)
    }
}

const fn argb(color: u32) -> u32 {
    0xff00_0000 | color
}

pub(crate) const fn colorref(color: u32) -> COLORREF {
    COLORREF(((color & 0xff) << 16) | (color & 0xff00) | ((color >> 16) & 0xff))
}
