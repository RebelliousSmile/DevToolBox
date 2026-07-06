//! Fluent-style card renderer for owner-draw action buttons (Phase 2).
//!
//! This module provides the GDI painting logic for action cards, keeping all
//! unsafe Win32/GDI calls isolated and the pure geometry helpers unit-testable.
//!
//! ## Double-buffering
//!
//! All painting goes through a per-call memory DC (`CreateCompatibleDC` +
//! `CreateCompatibleBitmap`) to eliminate hover-transition flicker.  The final
//! `BitBlt` copies the off-screen buffer to the real `DRAWITEMSTRUCT.hDC`.
//!
//! ## Cached vs. per-paint GDI objects
//!
//! - **Cached** (created once in `UiHost::new`, freed in `Drop`):
//!   `HFONT` (label + emoji), fill/border `HBRUSH` constants.
//! - **Per-paint** (created and destroyed inside `paint_card`):
//!   `CreateRoundRectRgn` region, compatible-DC bitmap, memory DC.
//!
//! ## Light-theme color constants
//!
//! Defined as module-level constants — change here for the whole theme.

use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateRoundRectRgn, CreateSolidBrush,
    DeleteDC, DeleteObject, DrawTextW, FillRgn, FrameRgn, SelectObject, SetBkMode, SetTextColor,
    DT_CENTER, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, HBITMAP, HBRUSH, HDC, HFONT, HGDIOBJ,
    SRCCOPY, TRANSPARENT,
};

// ---------------------------------------------------------------------------
// ODS_* flags (from Win32::UI::Controls — already enabled via Win32_UI_Controls)
// ---------------------------------------------------------------------------

/// ODS_SELECTED: the item is selected (pressed state for buttons).
pub const ODS_SELECTED: u32 = 0x0001;
/// ODS_FOCUS: the item has keyboard focus.
pub const ODS_FOCUS: u32 = 0x0010;

// ---------------------------------------------------------------------------
// Light-theme constants
// ---------------------------------------------------------------------------

/// Card background fill — normal state (very light gray, near-white).
pub const COLOR_FILL_NORMAL: COLORREF = COLORREF(0x00F5F5F5);
/// Card background fill — hover state (slightly blue-tinted; Phase 3).
pub const COLOR_FILL_HOVER: COLORREF = COLORREF(0x00E8F0FE);
/// Card background fill — pressed/selected state (medium blue-tinted).
pub const COLOR_FILL_PRESSED: COLORREF = COLORREF(0x00D0E4FD);
/// Card border — normal state (light gray).
pub const COLOR_BORDER_NORMAL: COLORREF = COLORREF(0x00DEDEDE);
/// Card border — hover/focus state (accent blue, Fluent-style).
pub const COLOR_BORDER_FOCUS: COLORREF = COLORREF(0x000078D4);
/// Focus ring accent color (drawn as a second inset frame).
pub const COLOR_FOCUS_RING: COLORREF = COLORREF(0x000078D4);
/// Label text color — normal (near-black for light theme).
pub const COLOR_TEXT: COLORREF = COLORREF(0x00202020);
/// Label text color — pressed (slightly darker).
pub const COLOR_TEXT_PRESSED: COLORREF = COLORREF(0x00000000);

/// Corner radius (pixels) for rounded cards.
pub const CORNER_RADIUS: i32 = 8;
/// Border width (pixels) — used as the FrameRgn brush extent.
pub const BORDER_WIDTH: i32 = 1;
/// Padding (pixels) inside the card for icon/label content.
pub const INNER_PAD: i32 = 8;
/// Proportion of card height reserved for the icon area (0.55 = 55 %).
/// Applied in `split_rects`.
const ICON_HEIGHT_RATIO_NUM: i32 = 55;
const ICON_HEIGHT_RATIO_DEN: i32 = 100;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Visual state for a single card.
///
/// All three flags are independent and combinable — e.g. a card may be both
/// focused and hovered simultaneously.
///
/// * `is_hot`     — mouse is over the card (Phase 3, stubbed `false` in Phase 2).
/// * `is_pressed` — button is pressed (`ODS_SELECTED` from `DRAWITEMSTRUCT.itemState`).
/// * `is_focused` — button has keyboard focus (`ODS_FOCUS`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CardState {
    pub is_hot: bool,
    pub is_pressed: bool,
    pub is_focused: bool,
}

impl CardState {
    /// Normal state: nothing active.  Used in tests and by Phase 3 hover logic.
    #[allow(dead_code)]
    pub const NORMAL: Self = Self {
        is_hot: false,
        is_pressed: false,
        is_focused: false,
    };
}

/// Icon source for an action card.
///
/// The enum is owned (no lifetime parameter) so it can be stored in
/// `HashMap<u16, IconSource>` on `UiHost` without propagating a lifetime to
/// the thread-local `HOST`.
///
/// HBITMAP handle ownership is with `id_to_icon` on `UiHost`; `Drop` for
/// `UiHost` calls `DeleteObject` for each `Bitmap` variant.
pub enum IconSource {
    /// A decoded raster icon as a Win32 HBITMAP.
    ///
    /// Blitted centered into the icon area via a compatible memory DC.
    Bitmap(HBITMAP),
    /// An emoji or Unicode glyph string.
    ///
    /// Rendered with `DrawTextW` using the "Segoe UI Emoji" face.
    Emoji(String),
    /// No icon available; label fills the full card height.
    NoIcon,
}

// ---------------------------------------------------------------------------
// Pure geometry helpers (unit-tested)
// ---------------------------------------------------------------------------

/// Split a card rectangle into (icon_rect, label_rect) based on the icon
/// height ratio constant.
///
/// When `has_icon` is `false` (e.g. `IconSource::NoIcon`), the label rect
/// covers the entire card and the icon rect is zeroed.
///
/// The split is based solely on the card height — both sub-rects share the
/// full card width.  `INNER_PAD` is subtracted from all four sides before
/// splitting, so neither sub-rect touches the card border.
///
/// Returns `(icon_rect, label_rect)`.
pub fn split_rects(card: RECT, has_icon: bool) -> (RECT, RECT) {
    let left = card.left + INNER_PAD;
    let right = card.right - INNER_PAD;
    let top = card.top + INNER_PAD;
    let bottom = card.bottom - INNER_PAD;

    let total_h = (bottom - top).max(0);

    if !has_icon || total_h == 0 {
        // Clamp bottom to at least top so the rect is never inverted (e.g. zero-height card).
        let clamped_bottom = bottom.max(top);
        let label_rect = RECT {
            left,
            right,
            top,
            bottom: clamped_bottom,
        };
        return (
            RECT {
                left,
                right,
                top,
                bottom: top,
            },
            label_rect,
        );
    }

    let icon_h = (total_h * ICON_HEIGHT_RATIO_NUM / ICON_HEIGHT_RATIO_DEN).max(1);
    let icon_bottom = top + icon_h;
    let label_gap = 4; // small gap between icon and label areas

    let icon_rect = RECT {
        left,
        right,
        top,
        bottom: icon_bottom,
    };
    let label_rect = RECT {
        left,
        right,
        top: (icon_bottom + label_gap).min(bottom),
        bottom,
    };

    (icon_rect, label_rect)
}

/// Center a `(w, h)` box inside `container`, clamped to container bounds.
///
/// Returns a `RECT` with the box centered, or the container rect if the box
/// is larger than the container.
pub fn center_rect_in(container: RECT, w: i32, h: i32) -> RECT {
    let cw = (container.right - container.left).max(0);
    let ch = (container.bottom - container.top).max(0);
    let bw = w.min(cw);
    let bh = h.min(ch);
    let dx = (cw - bw) / 2;
    let dy = (ch - bh) / 2;
    RECT {
        left: container.left + dx,
        top: container.top + dy,
        right: container.left + dx + bw,
        bottom: container.top + dy + bh,
    }
}

// ---------------------------------------------------------------------------
// Cached GDI brush helper
// ---------------------------------------------------------------------------

/// Create a solid-color HBRUSH.  Returns a null brush on failure.
///
/// The caller owns the returned handle and must call `DeleteObject` when done.
pub fn make_brush(color: COLORREF) -> HBRUSH {
    // Safety: CreateSolidBrush always succeeds for a valid COLORREF; if it
    // somehow returns null the handle is HBRUSH(0) which is safe to pass to
    // FrameRgn/FillRgn (they silently do nothing).
    unsafe { CreateSolidBrush(color) }
}

// ---------------------------------------------------------------------------
// Main card painter
// ---------------------------------------------------------------------------

/// Paint a Fluent-style card into `hdc`.
///
/// All GDI work happens in an off-screen memory DC to eliminate flicker;
/// the result is BitBlt'd to `hdc` at the end.
///
/// # Parameters
///
/// * `hdc`        — the real device context from `DRAWITEMSTRUCT.hDC`.
/// * `rect`       — the card's bounding rectangle from `DRAWITEMSTRUCT.rcItem`.
/// * `state`      — the card's current visual state.
/// * `label_font` — cached HFONT for the card label (Segoe UI).
/// * `emoji_font` — cached HFONT for emoji glyphs (Segoe UI Emoji).
/// * `fill_brush` — cached HBRUSH for the card fill (pre-selected for state).
/// * `border_brush` — cached HBRUSH for the card border.
/// * `icon`       — icon source variant to render in the upper area.
/// * `label`      — card label text.
///
/// # Safety
///
/// `hdc` must be a valid Win32 device context.  `label_font` and `emoji_font`
/// must be valid HFONT handles (non-null).
#[allow(clippy::too_many_arguments)]
pub unsafe fn paint_card(
    hdc: HDC,
    rect: RECT,
    state: CardState,
    label_font: HFONT,
    emoji_font: HFONT,
    fill_brush: HBRUSH,
    border_brush: HBRUSH,
    icon: &IconSource,
    label: &str,
) {
    let w = rect.right - rect.left;
    let h = rect.bottom - rect.top;
    if w <= 0 || h <= 0 {
        return;
    }

    // ---- Double-buffer setup ----
    let mem_dc = CreateCompatibleDC(hdc);
    if mem_dc.is_invalid() {
        return;
    }
    let mem_bmp = CreateCompatibleBitmap(hdc, w, h);
    if mem_bmp.is_invalid() {
        let _ = DeleteDC(mem_dc);
        return;
    }
    // Select the back-buffer bitmap; save the original so we can restore it.
    let old_bmp = SelectObject(mem_dc, HGDIOBJ(mem_bmp.0));

    // Work in local (0,0)-relative coordinates on mem_dc.
    let local_rect = RECT {
        left: 0,
        top: 0,
        right: w,
        bottom: h,
    };

    // ---- Background erase ----
    // Paint the container background color (match container class brush).
    // We use COLOR_FILL_NORMAL as the base; the rounded region will cover it.
    let bg_brush = make_brush(COLORREF(0x00FFFFFF));
    windows::Win32::Graphics::Gdi::FillRect(mem_dc, &local_rect, bg_brush);
    let _ = DeleteObject(HGDIOBJ(bg_brush.0));

    // ---- Rounded card ----
    // CreateRoundRectRgn is per-paint (not cacheable — it embeds coords).
    let rgn = CreateRoundRectRgn(0, 0, w, h, CORNER_RADIUS, CORNER_RADIUS);

    // Fill with state-appropriate brush (passed in from UiHost cache).
    FillRgn(mem_dc, rgn, fill_brush);

    // Border: FrameRgn draws a border of width BORDER_WIDTH pixels using the
    // brush.  For focus state, draw a second, thicker focus ring.
    FrameRgn(mem_dc, rgn, border_brush, BORDER_WIDTH, BORDER_WIDTH);

    if state.is_focused {
        // Draw an inner focus ring (2 px inset, accent color).
        let focus_brush = make_brush(COLOR_FOCUS_RING);
        let focus_rgn =
            CreateRoundRectRgn(2, 2, w - 2, h - 2, CORNER_RADIUS - 2, CORNER_RADIUS - 2);
        FrameRgn(mem_dc, focus_rgn, focus_brush, 2, 2);
        let _ = DeleteObject(HGDIOBJ(focus_rgn.0));
        let _ = DeleteObject(HGDIOBJ(focus_brush.0));
    }

    // ---- Icon / label geometry ----
    let has_icon = !matches!(icon, IconSource::NoIcon);
    let (icon_rect, label_rect) = split_rects(local_rect, has_icon);

    // Select the correct fill state for text color.
    let text_color = if state.is_pressed {
        COLOR_TEXT_PRESSED
    } else {
        COLOR_TEXT
    };

    // ---- Icon rendering ----
    match icon {
        IconSource::Bitmap(hbitmap) => {
            blit_centered_bitmap(mem_dc, icon_rect, *hbitmap);
        }
        IconSource::Emoji(glyph) => {
            draw_emoji(mem_dc, icon_rect, emoji_font, glyph, text_color);
        }
        IconSource::NoIcon => {}
    }

    // ---- Label rendering ----
    draw_label(mem_dc, label_rect, label_font, label, text_color);

    // ---- Blit to real DC ----
    let _ = BitBlt(hdc, rect.left, rect.top, w, h, mem_dc, 0, 0, SRCCOPY);

    // ---- Cleanup memory DC resources ----
    let _ = DeleteObject(HGDIOBJ(rgn.0));
    SelectObject(mem_dc, old_bmp);
    let _ = DeleteObject(HGDIOBJ(mem_bmp.0));
    let _ = DeleteDC(mem_dc);
}

// ---------------------------------------------------------------------------
// Private rendering helpers
// ---------------------------------------------------------------------------

/// Blit an HBITMAP centered inside `dest_rect` in `hdc`.
///
/// Creates a temporary compatible memory DC, selects the bitmap, copies the
/// centered portion via StretchBlt (fill dest rect).
unsafe fn blit_centered_bitmap(hdc: HDC, dest_rect: RECT, hbitmap: HBITMAP) {
    use windows::Win32::Graphics::Gdi::{GetObjectW, BITMAP, HGDIOBJ};

    let dw = dest_rect.right - dest_rect.left;
    let dh = dest_rect.bottom - dest_rect.top;
    if dw <= 0 || dh <= 0 {
        return;
    }

    // Get bitmap dimensions.
    let mut bmp: BITMAP = std::mem::zeroed();
    let got = GetObjectW(
        HGDIOBJ(hbitmap.0),
        std::mem::size_of::<BITMAP>() as i32,
        Some(&mut bmp as *mut BITMAP as *mut _),
    );
    if got == 0 {
        return;
    }
    let bw = bmp.bmWidth;
    let bh = bmp.bmHeight;
    if bw <= 0 || bh <= 0 {
        return;
    }

    // Compute centered destination rectangle (keep aspect, fit in dest_rect).
    let scale_x = (dw as f32) / (bw as f32);
    let scale_y = (dh as f32) / (bh as f32);
    let scale = scale_x.min(scale_y);
    let draw_w = ((bw as f32 * scale) as i32).max(1);
    let draw_h = ((bh as f32 * scale) as i32).max(1);
    let dest = center_rect_in(dest_rect, draw_w, draw_h);

    let bmp_dc = CreateCompatibleDC(hdc);
    if bmp_dc.is_invalid() {
        return;
    }
    let old_obj = SelectObject(bmp_dc, HGDIOBJ(hbitmap.0));

    let _ = windows::Win32::Graphics::Gdi::StretchBlt(
        hdc,
        dest.left,
        dest.top,
        dest.right - dest.left,
        dest.bottom - dest.top,
        bmp_dc,
        0,
        0,
        bw,
        bh,
        SRCCOPY,
    );

    SelectObject(bmp_dc, old_obj);
    let _ = DeleteDC(bmp_dc);
}

/// Render an emoji/glyph string centered in `rect` using `emoji_font`.
unsafe fn draw_emoji(hdc: HDC, rect: RECT, emoji_font: HFONT, glyph: &str, text_color: COLORREF) {
    if rect.bottom <= rect.top || rect.right <= rect.left {
        return;
    }
    let old_font = SelectObject(hdc, HGDIOBJ(emoji_font.0));
    SetBkMode(hdc, TRANSPARENT);
    SetTextColor(hdc, text_color);

    let mut text_w: Vec<u16> = glyph.encode_utf16().collect();
    // DrawTextW needs a non-null pointer; push a null terminator for safety.
    text_w.push(0);
    let glyph_len = text_w.len() - 1; // length without the null terminator
    let mut r = rect;
    DrawTextW(
        hdc,
        &mut text_w[..glyph_len],
        &mut r,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
    );

    SelectObject(hdc, old_font);
}

/// Render the card label string centered (multi-line word-wrap) in `rect`.
unsafe fn draw_label(hdc: HDC, rect: RECT, label_font: HFONT, label: &str, text_color: COLORREF) {
    if rect.bottom <= rect.top || rect.right <= rect.left {
        return;
    }
    let old_font = SelectObject(hdc, HGDIOBJ(label_font.0));
    SetBkMode(hdc, TRANSPARENT);
    SetTextColor(hdc, text_color);

    let mut text_w: Vec<u16> = label.encode_utf16().collect();
    text_w.push(0);
    let label_len = text_w.len() - 1; // length without the null terminator
    let mut r = rect;
    DrawTextW(
        hdc,
        &mut text_w[..label_len],
        &mut r,
        DT_CENTER | DT_VCENTER | DT_WORDBREAK,
    );

    SelectObject(hdc, old_font);
}

// ---------------------------------------------------------------------------
// Unit tests (pure geometry only — no GDI)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    // --- split_rects ---

    #[test]
    fn split_rects_no_icon_label_fills_padded_card() {
        let card = rect(0, 0, 100, 80);
        let (icon_rect, label_rect) = split_rects(card, false);
        // Icon rect should be zero-height (top == bottom).
        assert_eq!(
            icon_rect.top, icon_rect.bottom,
            "icon rect must be zero-height when no icon"
        );
        // Label rect should equal the padded card area.
        assert_eq!(label_rect.left, INNER_PAD);
        assert_eq!(label_rect.right, 100 - INNER_PAD);
        assert_eq!(label_rect.top, INNER_PAD);
        assert_eq!(label_rect.bottom, 80 - INNER_PAD);
    }

    #[test]
    fn split_rects_with_icon_icon_is_above_label() {
        let card = rect(0, 0, 100, 100);
        let (icon_rect, label_rect) = split_rects(card, true);
        // Icon is in the upper portion.
        assert!(
            icon_rect.top < icon_rect.bottom,
            "icon rect must have positive height"
        );
        // Label starts below icon bottom.
        assert!(
            label_rect.top > icon_rect.top,
            "label must start below icon top"
        );
        // Label bottom is padded.
        assert_eq!(label_rect.bottom, 100 - INNER_PAD);
    }

    #[test]
    fn split_rects_with_icon_roughly_55_percent_icon() {
        let card = rect(0, 0, 100, 100);
        let (icon_rect, label_rect) = split_rects(card, true);
        let available_h = 100 - 2 * INNER_PAD; // padded height
        let icon_h = icon_rect.bottom - icon_rect.top;
        // Icon should be approximately 55 % of the padded height (within rounding).
        let expected = available_h * ICON_HEIGHT_RATIO_NUM / ICON_HEIGHT_RATIO_DEN;
        assert!(
            (icon_h - expected).abs() <= 1,
            "icon_h={icon_h} expected~={expected}"
        );
        // Label must not overlap icon area (after the gap).
        assert!(
            label_rect.top >= icon_rect.bottom,
            "label must start at or after icon bottom"
        );
    }

    #[test]
    fn split_rects_both_halves_stay_within_card() {
        let card = rect(10, 20, 110, 120);
        let (icon_rect, label_rect) = split_rects(card, true);
        // Both sub-rects stay within card bounds.
        assert!(icon_rect.left >= card.left + INNER_PAD);
        assert!(icon_rect.right <= card.right - INNER_PAD);
        assert!(icon_rect.top >= card.top + INNER_PAD);
        assert!(icon_rect.bottom <= card.bottom - INNER_PAD);
        assert!(label_rect.left >= card.left + INNER_PAD);
        assert!(label_rect.right <= card.right - INNER_PAD);
        assert!(label_rect.top >= card.top + INNER_PAD);
        assert!(label_rect.bottom <= card.bottom - INNER_PAD);
    }

    #[test]
    fn split_rects_zero_height_card_does_not_panic() {
        let card = rect(0, 0, 100, 0);
        let (icon_rect, label_rect) = split_rects(card, true);
        // Must not panic; both rects should be degenerate but non-negative.
        assert!(icon_rect.bottom >= icon_rect.top);
        assert!(label_rect.bottom >= label_rect.top);
    }

    // --- center_rect_in ---

    #[test]
    fn center_rect_in_centers_smaller_box() {
        let container = rect(0, 0, 100, 100);
        let centered = center_rect_in(container, 40, 20);
        assert_eq!(centered.left, 30);
        assert_eq!(centered.right, 70);
        assert_eq!(centered.top, 40);
        assert_eq!(centered.bottom, 60);
    }

    #[test]
    fn center_rect_in_clamps_to_container() {
        let container = rect(0, 0, 50, 50);
        let centered = center_rect_in(container, 200, 200);
        // Box clamps to container size, so centered at (0,0)-(50,50).
        assert_eq!(centered.left, 0);
        assert_eq!(centered.top, 0);
        assert_eq!(centered.right, 50);
        assert_eq!(centered.bottom, 50);
    }

    #[test]
    fn center_rect_in_offset_container() {
        let container = rect(10, 20, 110, 120);
        let centered = center_rect_in(container, 20, 20);
        // container is 100x100, box is 20x20 → offset (40,40) from container origin.
        assert_eq!(centered.left, 50);
        assert_eq!(centered.top, 60);
        assert_eq!(centered.right, 70);
        assert_eq!(centered.bottom, 80);
    }

    // --- CardState ---

    #[test]
    fn card_state_normal_is_all_false() {
        let s = CardState::NORMAL;
        assert!(!s.is_hot);
        assert!(!s.is_pressed);
        assert!(!s.is_focused);
    }

    #[test]
    fn card_state_flags_are_independent() {
        let s = CardState {
            is_hot: true,
            is_pressed: true,
            is_focused: false,
        };
        assert!(s.is_hot);
        assert!(s.is_pressed);
        assert!(!s.is_focused);
    }
}
