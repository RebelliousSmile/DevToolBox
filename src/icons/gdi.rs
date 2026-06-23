//! Win32/GDI bitmap bridge — RGBA buffer -> HBITMAP -> native BUTTON.
//!
//! All unsafe FFI is isolated here. This module is `#[cfg(windows)]` and is
//! excluded from the pure unit-test surface (Decision D6).
//!
//! # Safety invariants (documented per unsafe block below)
//! - `CreateDIBSection`: the returned pointer is valid as long as the HBITMAP
//!   is not deleted. We copy into it immediately and never hold the pointer past
//!   that copy, so there is no aliasing issue.
//! - `SetDIBits`: called immediately after `CreateDIBSection` before any other
//!   operation on the HDC, which satisfies the Win32 requirement.
//! - `set_button_bitmap`: the HWND must be a valid BUTTON created by the caller;
//!   callers (UiHost) ensure this.
//! - `delete_bitmap`: the HBITMAP must have been returned by `rgba_to_hbitmap`
//!   and must not have been selected into any DC at the time of deletion.

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS, HBITMAP, HGDIOBJ,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, SendMessageW, SetWindowLongPtrW, BM_SETIMAGE, GWL_STYLE, IMAGE_BITMAP,
};

use crate::icons::DecodedIcon;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors from the GDI bitmap bridge.
#[derive(Debug)]
pub enum GdiError {
    /// `CreateDIBSection` returned a null handle.
    CreateDibFailed,
}

impl std::fmt::Display for GdiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GdiError::CreateDibFailed => write!(f, "CreateDIBSection returned null"),
        }
    }
}

impl std::error::Error for GdiError {}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Convert a decoded RGBA icon into a Win32 `HBITMAP`.
///
/// Uses a top-down 32bpp `BI_RGB` DIB. The `image` crate produces RGBA order;
/// Win32 DIBs expect BGRA — this function performs the channel swap on copy.
///
/// # Errors
/// Returns [`GdiError`] if `CreateDIBSection` or `SetDIBits` fails.
///
/// # Safety
/// The returned `HBITMAP` must be freed with [`delete_bitmap`] when no longer
/// needed. Do not select it into a DC when deleting.
pub fn rgba_to_hbitmap(icon: &DecodedIcon) -> Result<HBITMAP, GdiError> {
    let width = icon.width as i32;
    let height = icon.height as i32;

    // Build a top-down 32bpp BITMAPINFO.
    // Top-down: negative height in BITMAPINFOHEADER (Win32 convention).
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height, // negative = top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [Default::default()],
    };

    let mut ppv_bits: *mut std::ffi::c_void = std::ptr::null_mut();

    // Safety: CreateCompatibleDC(None) creates a memory DC compatible with the
    // screen. We delete it immediately after use.
    let hdc = unsafe { CreateCompatibleDC(None) };

    // Safety: CreateDIBSection allocates a DIB section and returns a pointer to
    // the pixel data in `ppv_bits`. We copy into it immediately before any other
    // DC operation and never alias the pointer after that.
    let hbitmap = unsafe { CreateDIBSection(hdc, &bmi, DIB_RGB_COLORS, &mut ppv_bits, None, 0) };

    // Release the temporary DC — we only needed it for CreateDIBSection.
    if !hdc.is_invalid() {
        // Safety: hdc was created by CreateCompatibleDC above.
        unsafe {
            let _ = DeleteDC(hdc);
        }
    }

    let hbitmap = match hbitmap {
        Ok(h) if !h.is_invalid() => h,
        _ => return Err(GdiError::CreateDibFailed),
    };

    if ppv_bits.is_null() {
        // Safety: hbitmap is valid but ppv_bits is null — unexpected; clean up.
        unsafe {
            let _ = DeleteObject(HGDIOBJ(hbitmap.0));
        }
        return Err(GdiError::CreateDibFailed);
    }

    // Copy pixels with RGBA -> BGRA channel swap.
    // Safety: ppv_bits points to a buffer of width * height * 4 bytes allocated
    // by CreateDIBSection. We write exactly that many bytes, one pixel at a time.
    let rgba = &icon.rgba;
    let pixel_count = (icon.width * icon.height) as usize;
    let dst = ppv_bits as *mut u8;
    for i in 0..pixel_count {
        let src_base = i * 4;
        let r = rgba[src_base];
        let g = rgba[src_base + 1];
        let b = rgba[src_base + 2];
        let a = rgba[src_base + 3];
        unsafe {
            // Win32 DIB pixel layout (little-endian): B G R A
            *dst.add(src_base) = b;
            *dst.add(src_base + 1) = g;
            *dst.add(src_base + 2) = r;
            *dst.add(src_base + 3) = a;
        }
    }

    Ok(hbitmap)
}

/// Set a native Win32 BUTTON to display `hbitmap` as its image.
///
/// Adds `BS_BITMAP` to the button's window style and sends `BM_SETIMAGE`.
///
/// # Safety
/// `hwnd` must be a valid BUTTON window handle. `hbitmap` must be a valid
/// HBITMAP not yet selected into any DC.
pub fn set_button_bitmap(hwnd: HWND, hbitmap: HBITMAP) {
    const BS_BITMAP: u32 = 0x0080;

    // Safety: hwnd is a valid Win32 BUTTON; GetWindowLongPtrW / SetWindowLongPtrW
    // are safe to call on a valid HWND with GWL_STYLE.
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        SetWindowLongPtrW(hwnd, GWL_STYLE, (style | BS_BITMAP) as isize);
        // Safety: BM_SETIMAGE with IMAGE_BITMAP requires WPARAM = IMAGE_BITMAP (0)
        // and LPARAM = HBITMAP handle value; this is the documented message contract.
        SendMessageW(
            hwnd,
            BM_SETIMAGE,
            WPARAM(IMAGE_BITMAP.0 as usize),
            LPARAM(hbitmap.0),
        );
    }
}

/// Delete a GDI bitmap handle created by [`rgba_to_hbitmap`].
///
/// Wraps `DeleteObject`. Safe to call only once per handle.
///
/// # Safety
/// `hbitmap` must not be selected into any DC at the time of the call.
pub fn delete_bitmap(hbitmap: HBITMAP) {
    // Safety: hbitmap was created by rgba_to_hbitmap; caller guarantees it is
    // not currently selected into a DC (UiHost clears bitmaps before any rebuild).
    unsafe {
        let _ = DeleteObject(HGDIOBJ(hbitmap.0));
    }
}
