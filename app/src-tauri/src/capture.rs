//! Screen capture around the cursor, cursor position and the foreground
//! window title. Pixels only: no process handles, no memory reads, no input.

use anyhow::{anyhow, Context, Result};
use image::RgbaImage;

/// Capture box in physical pixels; the cursor sits at 68 % from the left and
/// 45 % from the top because Last Epoch draws a bag item's tooltip to the
/// left of the cursor and it can be 900 px tall.
pub const BOX_W: u32 = 2400;
pub const BOX_H: u32 = 1500;
pub const CURSOR_FRAC_X: f32 = 0.78;
pub const CURSOR_FRAC_Y: f32 = 0.45;

pub struct Capture {
    pub image: RgbaImage,
    /// absolute physical position of the crop's top-left corner
    pub origin: (i32, i32),
    /// cursor position relative to the crop
    pub cursor_in_crop: (f32, f32),
    /// absolute physical cursor position
    pub cursor: (i32, i32),
    pub monitor: MonitorInfo,
}

#[derive(Debug, Clone)]
pub struct MonitorInfo {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
}

#[cfg(windows)]
pub fn cursor_pos() -> Result<(i32, i32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point) }.context("GetCursorPos")?;
    Ok((point.x, point.y))
}

#[cfg(windows)]
pub fn foreground_title() -> String {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW};
    unsafe {
        let hwnd = GetForegroundWindow();
        let mut buf = [0u16; 256];
        let len = GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..len.max(0) as usize])
    }
}

#[cfg(not(windows))]
pub fn cursor_pos() -> Result<(i32, i32)> {
    Err(anyhow!("cursor position is only implemented on Windows"))
}

#[cfg(not(windows))]
pub fn foreground_title() -> String {
    String::new()
}

pub fn game_is_foreground() -> bool {
    foreground_title().to_lowercase().contains("last epoch")
}

/// Monitor under the cursor.
pub fn monitor_at(x: i32, y: i32) -> Result<(xcap::Monitor, MonitorInfo)> {
    let monitor = xcap::Monitor::from_point(x, y).map_err(|e| anyhow!("no monitor at cursor: {e}"))?;
    let info = MonitorInfo {
        x: monitor.x().map_err(|e| anyhow!("{e}"))?,
        y: monitor.y().map_err(|e| anyhow!("{e}"))?,
        width: monitor.width().map_err(|e| anyhow!("{e}"))?,
        height: monitor.height().map_err(|e| anyhow!("{e}"))?,
        scale: monitor.scale_factor().map_err(|e| anyhow!("{e}"))?,
    };
    Ok((monitor, info))
}

/// Grab the monitor under the cursor and crop the box around the cursor.
pub fn capture_around_cursor() -> Result<Capture> {
    let (cx, cy) = cursor_pos()?;
    let (monitor, info) = monitor_at(cx, cy)?;
    let full = monitor.capture_image().map_err(|e| anyhow!("screen capture failed: {e}"))?;
    let (mw, mh) = full.dimensions();
    // cursor relative to the monitor image (physical pixels)
    let rel_x = ((cx - info.x) as f32 * (mw as f32 / info.width.max(1) as f32)) as i32;
    let rel_y = ((cy - info.y) as f32 * (mh as f32 / info.height.max(1) as f32)) as i32;
    let w = BOX_W.min(mw);
    let h = BOX_H.min(mh);
    let left = (rel_x - (w as f32 * CURSOR_FRAC_X) as i32).clamp(0, (mw - w) as i32) as u32;
    let top = (rel_y - (h as f32 * CURSOR_FRAC_Y) as i32).clamp(0, (mh - h) as i32) as u32;
    let crop = image::imageops::crop_imm(&full, left, top, w, h).to_image();
    Ok(Capture {
        image: crop,
        origin: (info.x + left as i32, info.y + top as i32),
        cursor_in_crop: ((rel_x - left as i32) as f32, (rel_y - top as i32) as f32),
        cursor: (cx, cy),
        monitor: info,
    })
}
