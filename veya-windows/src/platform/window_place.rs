//! Place a logical-size window in the cursor monitor's physical work area.

use windows::core::{s, w, HRESULT};
use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::{
    GetDC, GetDeviceCaps, GetMonitorInfoW, MonitorFromPoint, ReleaseDC, HMONITOR, LOGPIXELSX,
    MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

type GetDpiForMonitorFn = unsafe extern "system" fn(HMONITOR, i32, *mut u32, *mut u32) -> HRESULT;

/// Effective DPI of `monitor`.
///
/// `GetDpiForMonitor` lives in shcore.dll and only exists on Windows 8.1+, so it is resolved at
/// runtime. A static import would stop the process from loading on Windows 7. When it is missing
/// (or fails) fall back to the system DPI, then to 96.
unsafe fn monitor_dpi(monitor: HMONITOR) -> u32 {
    if let Ok(lib) = LoadLibraryW(w!("shcore.dll")) {
        if let Some(proc_addr) = GetProcAddress(lib, s!("GetDpiForMonitor")) {
            let get_dpi = std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                GetDpiForMonitorFn,
            >(proc_addr);
            let (mut dpi_x, mut dpi_y) = (0u32, 0u32);
            // 0 == MDT_EFFECTIVE_DPI
            if get_dpi(monitor, 0, &mut dpi_x, &mut dpi_y).is_ok() && dpi_x != 0 {
                return dpi_x;
            }
        }
    }
    let hdc = GetDC(None);
    let dpi = GetDeviceCaps(Some(hdc), LOGPIXELSX);
    ReleaseDC(None, hdc);
    if dpi > 0 {
        dpi as u32
    } else {
        96
    }
}

#[derive(Clone, Copy, Debug)]
pub struct StartupPlacement {
    pub logical_size: (f32, f32),
    pub physical_position: (f32, f32),
}

/// Fit a window in the cursor monitor's work area, then center it there.
pub fn startup_placement(window_width: f32, window_height: f32) -> Option<StartupPlacement> {
    unsafe {
        let mut cursor = POINT::default();
        GetCursorPos(&mut cursor).ok()?;
        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return None;
        }

        let dpi_x = monitor_dpi(monitor);
        let scale = if dpi_x == 0 { 1.0 } else { dpi_x as f32 / 96.0 };
        let work = info.rcWork;
        Some(fit_and_center(
            (work.left, work.top, work.right, work.bottom),
            (window_width, window_height),
            scale,
        ))
    }
}

fn fit_and_center(work: (i32, i32, i32, i32), size: (f32, f32), scale: f32) -> StartupPlacement {
    let (left, top, right, bottom) = work;
    let available_width = ((right - left) as f32 / scale).max(1.0);
    let available_height = ((bottom - top) as f32 / scale).max(1.0);
    let logical_size = (size.0.min(available_width), size.1.min(available_height));
    StartupPlacement {
        logical_size,
        physical_position: (
            left as f32 + (((right - left) as f32 - logical_size.0 * scale) / 2.0).max(0.0),
            top as f32 + (((bottom - top) as f32 - logical_size.1 * scale) / 2.0).max(0.0),
        ),
    }
}

/// Iced's `move_to` converts logical coordinates using the current window scale.
pub fn physical_to_window_logical(position: (f32, f32), current_scale: f32) -> (f32, f32) {
    let scale = if current_scale > 0.0 {
        current_scale
    } else {
        1.0
    };
    (position.0 / scale, position.1 / scale)
}

#[cfg(test)]
mod tests {
    use super::{fit_and_center, physical_to_window_logical};

    #[test]
    fn centers_on_scaled_secondary_monitor_with_negative_origin() {
        let placement = fit_and_center((-2560, 0, 0, 1400), (1080.0, 700.0), 1.5);
        let physical = placement.physical_position;
        assert_eq!(placement.logical_size, (1080.0, 700.0));
        assert_eq!(physical, (-2090.0, 175.0));
        assert_eq!(physical_to_window_logical(physical, 1.0), physical);
        let logical = physical_to_window_logical(physical, 1.5);
        assert!((logical.0 + 1393.3334).abs() < 0.001);
        assert!((logical.1 - 116.6667).abs() < 0.001);
    }

    #[test]
    fn oversized_window_fits_in_work_area() {
        let placement = fit_and_center((1920, 40, 2720, 640), (1080.0, 700.0), 1.0);
        assert_eq!(placement.logical_size, (800.0, 600.0));
        assert_eq!(placement.physical_position, (1920.0, 40.0));
    }

    #[test]
    fn high_dpi_work_area_reduces_height_and_keeps_taskbar_clear() {
        let placement = fit_and_center((0, 40, 1920, 1000), (1080.0, 700.0), 1.5);
        assert_eq!(placement.logical_size, (1080.0, 640.0));
        assert_eq!(placement.physical_position, (150.0, 40.0));
    }
}
