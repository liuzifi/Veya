//! Place a logical-size window in the cursor monitor's physical work area.

use windows::core::{s, w, HRESULT, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, POINT};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, GetDC, GetDeviceCaps, GetMonitorInfoW, MonitorFromPoint, ReleaseDC, HMONITOR,
    LOGPIXELSX, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Shell::{ITaskbarList, TaskbarList};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetCursorPos, GetForegroundWindow, GetGUIThreadInfo, GetWindowLongPtrW,
    GetWindowThreadProcessId, IsWindowVisible, SetLayeredWindowAttributes, SetWindowLongPtrW,
    SetWindowPos, GUITHREADINFO, GWL_EXSTYLE, LWA_ALPHA, SWP_FRAMECHANGED, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_TOOLWINDOW,
};

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

/// Anchor a window's top-left corner at a physical point, clamped into the
/// point's monitor work area so the panel never spills off-screen.
pub fn anchor_placement(
    anchor_physical: (f32, f32),
    window_width: f32,
    window_height: f32,
) -> Option<StartupPlacement> {
    unsafe {
        let point = POINT {
            x: anchor_physical.0 as i32,
            y: anchor_physical.1 as i32,
        };
        let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
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
        Some(fit_and_anchor(
            (work.left, work.top, work.right, work.bottom),
            (window_width, window_height),
            scale,
            anchor_physical,
        ))
    }
}

fn fit_and_anchor(
    work: (i32, i32, i32, i32),
    size: (f32, f32),
    scale: f32,
    anchor: (f32, f32),
) -> StartupPlacement {
    let (left, top, right, bottom) = work;
    let available_width = ((right - left) as f32 / scale).max(1.0);
    let available_height = ((bottom - top) as f32 / scale).max(1.0);
    let logical_size = (size.0.min(available_width), size.1.min(available_height));
    let phys_w = logical_size.0 * scale;
    let phys_h = logical_size.1 * scale;
    let x = anchor
        .0
        .clamp(left as f32, (right as f32 - phys_w).max(left as f32));
    let y = anchor
        .1
        .clamp(top as f32, (bottom as f32 - phys_h).max(top as f32));
    StartupPlacement {
        logical_size,
        physical_position: (x, y),
    }
}

/// Anchor the panel's top-left corner at the current cursor position.
pub fn cursor_anchor_placement(window_width: f32, window_height: f32) -> Option<StartupPlacement> {
    unsafe {
        let mut cursor = POINT::default();
        GetCursorPos(&mut cursor).ok()?;
        anchor_placement(
            (cursor.x as f32, cursor.y as f32),
            window_width,
            window_height,
        )
    }
}

/// Bottom-right corner of the caret (text insertion point) in the
/// foreground application, in physical screen coordinates.
///
/// Queries the foreground window's thread GUI state so the panel can open
/// right next to where the user is typing. Returns `None` when there is no
/// caret (desktop focus, terminals without caret reporting, ...).
pub fn caret_physical_position() -> Option<(f32, f32)> {
    unsafe {
        let foreground = GetForegroundWindow();
        let tid = GetWindowThreadProcessId(foreground, None);
        if tid == 0 {
            return None;
        }
        let mut gti = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        if GetGUIThreadInfo(tid, &mut gti).is_err() {
            return None;
        }
        if gti.hwndCaret.is_invalid() {
            return None;
        }
        // rcCaret is in the caret window's client coordinates; its bottom-right
        // corner is where the next typed character will appear.
        let mut point = POINT {
            x: gti.rcCaret.right,
            y: gti.rcCaret.bottom,
        };
        if !ClientToScreen(gti.hwndCaret, &mut point).as_bool() {
            return None;
        }
        Some((point.x as f32, point.y as f32))
    }
}

/// The single shared Veya window (manager and quick picker reuse one window).
/// Never change a same-titled window belonging to another process.
fn find_app_window() -> Option<HWND> {
    unsafe {
        let mut after = None;
        loop {
            let hwnd = FindWindowExW(None, after, PCWSTR::null(), w!("Veya")).ok()?;
            let mut process_id = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut process_id));
            if process_id == GetCurrentProcessId() {
                return Some(hwnd);
            }
            after = Some(hwnd);
        }
    }
}

fn taskbar_style(style: isize, visible: bool) -> isize {
    let unrelated = style & !((WS_EX_APPWINDOW.0 | WS_EX_TOOLWINDOW.0) as isize);
    unrelated
        | if visible {
            WS_EX_APPWINDOW.0 as isize
        } else {
            WS_EX_TOOLWINDOW.0 as isize
        }
}

/// Change the role of an existing, process-owned window; never create a window
/// or start a process. Quick/hidden windows are tools; only explicit manager
/// and settings windows belong in the taskbar and Alt+Tab.
///
/// Call on the window thread after Iced's mode/level changes: Winit can replace
/// the extended style during those operations. Shell notification also removes
/// a taskbar button left over from the window's previous manager role.
/// All APIs used here are available on Windows 7.
pub fn set_window_taskbar_visible(raw_hwnd: isize, visible: bool) -> bool {
    let hwnd = HWND(raw_hwnd as *mut core::ffi::c_void);
    unsafe {
        let mut process_id = 0;
        if hwnd.is_invalid()
            || GetWindowThreadProcessId(hwnd, Some(&mut process_id)) == 0
            || process_id != GetCurrentProcessId()
        {
            return false;
        }
        let current = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let wanted = taskbar_style(current, visible);
        let style_ok = if current == wanted {
            true
        } else {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, wanted);
            let refreshed = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
            refreshed.is_ok() && GetWindowLongPtrW(hwnd, GWL_EXSTYLE) == wanted
        };

        // Winit may already have initialized COM in another apartment. In that
        // case use that apartment, and only balance our own successful init.
        let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let shell_result = (|| -> windows::core::Result<()> {
            let taskbar: ITaskbarList = CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER)?;
            taskbar.HrInit()?;
            if visible && IsWindowVisible(hwnd).as_bool() {
                taskbar.AddTab(hwnd)
            } else {
                taskbar.DeleteTab(hwnd)
            }
        })();
        if initialized {
            CoUninitialize();
        }
        style_ok && shell_result.is_ok()
    }
}

/// Reapply the current role after a native resize/level update.
pub fn set_app_window_taskbar_visible(visible: bool) -> bool {
    find_app_window()
        .map(|hwnd| set_window_taskbar_visible(hwnd.0 as isize, visible))
        .unwrap_or(false)
}

/// Give the window per-pixel alpha via `WS_EX_LAYERED`. `opacity` is 0.0
/// (invisible) to 1.0 (opaque). Returns whether it was applied.
pub fn set_window_opacity(opacity: f32) -> bool {
    let Some(hwnd) = find_app_window() else {
        return false;
    };
    let alpha = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) | WS_EX_LAYERED.0 as isize;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style);
        // The colour key is ignored with LWA_ALPHA.
        SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA).is_ok()
    }
}

/// Remove the layered style again so the window renders fully opaque.
pub fn clear_window_opacity() -> bool {
    let Some(hwnd) = find_app_window() else {
        return false;
    };
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & !(WS_EX_LAYERED.0 as isize);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{fit_and_anchor, fit_and_center, physical_to_window_logical, taskbar_style};
    use windows::Win32::UI::WindowsAndMessaging::{
        WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    };

    #[test]
    fn quick_and_hidden_roles_remove_the_taskbar_flag_without_losing_other_styles() {
        let unrelated = (WS_EX_LAYERED.0 | WS_EX_TOPMOST.0) as isize;
        let manager = unrelated | WS_EX_APPWINDOW.0 as isize;
        let quick = taskbar_style(manager, false);
        assert_eq!(quick & WS_EX_APPWINDOW.0 as isize, 0);
        assert_ne!(quick & WS_EX_TOOLWINDOW.0 as isize, 0);
        assert_eq!(quick & unrelated, unrelated);
        assert_eq!(taskbar_style(quick, false), quick);
    }

    #[test]
    fn explicitly_opening_settings_restores_the_normal_taskbar_role() {
        let unrelated = (WS_EX_LAYERED.0 | WS_EX_TOPMOST.0) as isize;
        let manager = unrelated | WS_EX_APPWINDOW.0 as isize;
        let mut style = manager;
        for _ in 0..100 {
            style = taskbar_style(style, false);
            style = taskbar_style(style, true);
            assert_eq!(style, manager);
            assert_eq!(style & WS_EX_TOOLWINDOW.0 as isize, 0);
        }
    }

    #[test]
    fn rapid_quick_open_dismiss_reopen_round_trips_back_to_app_window() {
        // Phase 7 regression guard: opening the quick panel, dismissing it to
        // the tray, and reopening must end with WS_EX_APPWINDOW set and
        // WS_EX_TOOLWINDOW cleared, mirroring a fresh manager. A stale
        // tool-window flag left by the dismissed panel would make the shared
        // window vanish from the taskbar after the manager is restored.
        let unrelated = (WS_EX_LAYERED.0 | WS_EX_TOPMOST.0) as isize;
        let manager = unrelated | WS_EX_APPWINDOW.0 as isize;
        let mut style = manager;
        // open quick panel → tool window.
        style = taskbar_style(style, false);
        assert_ne!(style & WS_EX_TOOLWINDOW.0 as isize, 0);
        // dismiss to tray → stays a tool window.
        style = taskbar_style(style, false);
        // reopen quick → still a tool window.
        style = taskbar_style(style, false);
        // restore manager → app window again, unrelated bits intact.
        style = taskbar_style(style, true);
        assert_eq!(style, manager);
        assert_eq!(style & WS_EX_TOOLWINDOW.0 as isize, 0);
    }

    #[test]
    fn taskbar_changes_reject_an_invalid_window() {
        assert!(!super::set_window_taskbar_visible(0, false));
        assert!(!super::set_window_taskbar_visible(0, true));
    }

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

    #[test]
    fn anchored_window_clamps_into_work_area() {
        let placement = fit_and_anchor((0, 0, 1920, 1040), (440.0, 724.0), 1.0, (1900.0, 1000.0));
        assert_eq!(placement.logical_size, (440.0, 724.0));
        assert_eq!(placement.physical_position, (1480.0, 316.0));
        // Anchor far left of a negative-origin monitor stays inside.
        let placement = fit_and_anchor((-2560, 0, 0, 1400), (440.0, 724.0), 1.0, (-2600.0, -10.0));
        assert_eq!(placement.physical_position, (-2560.0, 0.0));
    }
}
