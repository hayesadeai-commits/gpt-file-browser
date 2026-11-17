//! Remember normal window bounds separately from maximized/minimized state.
//! Coordinates are physical desktop pixels; clamp to the nearest monitor's work
//! area (which excludes the taskbar) when a monitor or its resolution changes.
use serde::{Deserialize, Serialize};
use std::path::Path;
use tao::{
    dpi::{PhysicalPosition, PhysicalSize},
    window::Window,
};
use windows::Win32::{
    Foundation::RECT,
    Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect},
};

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct Placement {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    maximized: bool,
}

/// Use wide arithmetic for saved/untrusted coordinates; disconnected displays
/// can have negative origins, so clamping must not assume the primary display.
fn fit(mut p: Placement, work: RECT, center: bool) -> Placement {
    let w = (i64::from(work.right) - i64::from(work.left)).max(1);
    let h = (i64::from(work.bottom) - i64::from(work.top)).max(1);
    p.width = i64::from(p.width).clamp(1, w) as u32;
    p.height = i64::from(p.height).clamp(1, h) as u32;
    let x = if center {
        i64::from(work.left) + (w - i64::from(p.width)) / 2
    } else {
        i64::from(p.x)
    };
    let y = if center {
        i64::from(work.top) + (h - i64::from(p.height)) / 2
    } else {
        i64::from(p.y)
    };
    p.x = x.clamp(
        i64::from(work.left),
        i64::from(work.right) - i64::from(p.width),
    ) as i32;
    p.y = y.clamp(
        i64::from(work.top),
        i64::from(work.bottom) - i64::from(p.height),
    ) as i32;
    p
}

impl Placement {
    pub fn restore(window: &Window, root: &Path) -> Self {
        let pos = window.outer_position().unwrap_or_default();
        let size = window.outer_size();
        let saved = std::fs::read(root.join("window.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Self>(&b).ok())
            .filter(|p| p.width >= 200 && p.height >= 150 && p.width <= 32768 && p.height <= 32768);
        let mut p = saved.unwrap_or(Self {
            x: pos.x,
            y: pos.y,
            width: size.width,
            height: size.height,
            maximized: false,
        });
        let rect = RECT {
            left: p.x,
            top: p.y,
            right: p.x.saturating_add(p.width as i32),
            bottom: p.y.saturating_add(p.height as i32),
        };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: live stack structs have the sizes expected by the Windows API.
        if unsafe { GetMonitorInfoW(MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST), &mut info) }
            .as_bool()
        {
            p = fit(p, info.rcWork, saved.is_none());
        }
        // Remove the fixed minimum temporarily: a small/high-DPI display may have
        // less usable space than it. Account for title bar and borders in the fit.
        window.set_min_inner_size(None::<PhysicalSize<u32>>);
        window.set_outer_position(PhysicalPosition::new(p.x, p.y));
        let outer = window.outer_size();
        let inner = window.inner_size();
        let width = p
            .width
            .saturating_sub(outer.width.saturating_sub(inner.width))
            .max(1);
        let height = p
            .height
            .saturating_sub(outer.height.saturating_sub(inner.height))
            .max(1);
        window.set_inner_size(PhysicalSize::new(width, height));
        let scale = window.scale_factor();
        window.set_min_inner_size(Some(PhysicalSize::new(
            width.min((980.0 * scale) as u32),
            height.min((680.0 * scale) as u32),
        )));
        window.set_outer_position(PhysicalPosition::new(p.x, p.y));
        window.set_maximized(p.maximized);
        p
    }
    /// Startup check uses the real native frame, including title bar and taskbar.
    /// Maximized windows have invisible borders beyond the work area by design.
    pub fn is_on_screen(window: &Window) -> bool {
        if window.is_maximized() {
            return true;
        }
        let Ok(pos) = window.outer_position() else {
            return false;
        };
        let size = window.outer_size();
        let rect = RECT {
            left: pos.x,
            top: pos.y,
            right: pos.x.saturating_add(size.width as i32),
            bottom: pos.y.saturating_add(size.height as i32),
        };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: structs are initialized and valid for the duration of the API call.
        unsafe { GetMonitorInfoW(MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST), &mut info) }
            .as_bool()
            && rect.left >= info.rcWork.left
            && rect.top >= info.rcWork.top
            && rect.right <= info.rcWork.right
            && rect.bottom <= info.rcWork.bottom
    }
    /// Never overwrite normal bounds with the minimized offscreen coordinates.
    pub fn save(&mut self, window: &Window, root: &Path) {
        if window.is_minimized() {
            return;
        }
        self.maximized = window.is_maximized();
        if !self.maximized
            && let Ok(pos) = window.outer_position()
        {
            let size = window.outer_size();
            self.x = pos.x;
            self.y = pos.y;
            self.width = size.width;
            self.height = size.height;
        }
        if let Ok(bytes) = serde_json::to_vec(self) {
            let _ = std::fs::write(root.join("window.json"), bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fits_taskbar_disconnected_monitor_and_negative_origin() {
        let work = RECT {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1040,
        };
        let p = fit(
            Placement {
                x: 5000,
                y: 5000,
                width: 2500,
                height: 1400,
                maximized: false,
            },
            work,
            false,
        );
        assert_eq!((p.x, p.y, p.width, p.height), (-1920, 0, 1920, 1040));
        let p = fit(
            Placement {
                x: 0,
                y: 0,
                width: 1280,
                height: 720,
                maximized: false,
            },
            work,
            true,
        );
        assert_eq!((p.x, p.y), (-1600, 160));
    }
}
