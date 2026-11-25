//! On Windows 10 the accent layer a blurred window asks for covers the whole window
//! rectangle, while GPUI's client area keeps the system's left, right and bottom resize
//! insets. A see-through window then shows a bare acrylic band along those edges. This
//! subclass widens the client area onto the window rectangle and, for a resizable window,
//! answers the resize hit tests the removed frame used to serve. Windows 11 tints its acrylic
//! and covers the frame itself, so it is left alone.

use gpui::{Pixels, Size, Window, WindowVisibility, px, size};

/// Whether this is Windows 10 or older, by build number: 22000 is where Windows 11 starts.
#[cfg(target_os = "windows")]
pub(crate) fn legacy_windows() -> bool {
    use std::sync::OnceLock;
    use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
    use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;

    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| unsafe {
        let mut version = OSVERSIONINFOW {
            dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
            ..Default::default()
        };
        RtlGetVersion(&mut version);
        version.dwBuildNumber < 22000
    })
}

/// Widens a window's client area over the frame on Windows 10. `resizable` serves the frame
/// edges' hit tests too; a popup that cannot resize only needs the client area widened.
///
/// Returns the frame inset GPUI still adds to `Window::resize` on this platform. The client
/// area is widened after GPUI measured that inset, so a later resize request must subtract it
/// or the window stays one frame larger than the surface the view paints.
pub fn install(window: &Window, resizable: bool) -> Size<Pixels> {
    #[cfg(target_os = "windows")]
    {
        if !legacy_windows() {
            return size(px(0.), px(0.));
        }
        let Some(handle) = hwnd(window) else {
            return size(px(0.), px(0.));
        };
        use windows_sys::Win32::Foundation::RECT;
        use windows_sys::Win32::UI::Shell::SetWindowSubclass;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetClientRect, GetWindowRect, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE,
            SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
        };
        let scale = window.scale_factor();
        let content = window.viewport_size();
        let visible = window.visibility() == WindowVisibility::Visible;
        let inset = unsafe {
            let mut outer = RECT::default();
            let mut client = RECT::default();
            if GetWindowRect(handle, &mut outer) == 0 || GetClientRect(handle, &mut client) == 0 {
                return size(px(0.), px(0.));
            }
            size(
                px(((outer.right - outer.left) - (client.right - client.left)) as f32 / scale),
                px(((outer.bottom - outer.top) - (client.bottom - client.top)) as f32 / scale),
            )
        };
        unsafe {
            SetWindowSubclass(handle, Some(client_area), LEGACY_CLIENT_SUBCLASS, resizable as usize);
            // The frame was already measured without the subclass, so lay it out again.
            SetWindowPos(
                handle,
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            // A popup that was already laid out keeps its old outer size (and the material
            // shows in the widened frame) until it is sized again. Visible popups have a
            // usable content size here; hidden ones get theirs when they are first shown.
            if !resizable && visible {
                let width = (content.width.as_f32() * scale).round() as i32;
                let height = (content.height.as_f32() * scale).round() as i32;
                if width > 0 && height > 0 {
                    SetWindowPos(
                        handle,
                        std::ptr::null_mut(),
                        0,
                        0,
                        width,
                        height,
                        SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
            }
        }
        inset
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (window, resizable);
        size(px(0.), px(0.))
    }
}

#[cfg(target_os = "windows")]
const LEGACY_CLIENT_SUBCLASS: usize = 2;

#[cfg(target_os = "windows")]
fn hwnd(window: &Window) -> Option<*mut std::ffi::c_void> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return None;
    };
    Some(handle.hwnd.get() as *mut std::ffi::c_void)
}

/// Whether the window still carries its resize frame. Fullscreen drops `WS_THICKFRAME`, and a
/// frameless window has no insets to widen.
#[cfg(target_os = "windows")]
fn framed(handle: *mut std::ffi::c_void) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_STYLE, GetWindowLongPtrW, WS_THICKFRAME,
    };

    let style = unsafe { GetWindowLongPtrW(handle, GWL_STYLE) };
    style & WS_THICKFRAME as isize != 0
}

#[cfg(target_os = "windows")]
fn zoomed(handle: *mut std::ffi::c_void) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::IsZoomed;

    unsafe { IsZoomed(handle) != 0 }
}

/// The resize edge the point is over, now that the client area covers the frame. `None`
/// leaves the hit test to the rest of the chain, whose answers cover the title bar's drag
/// area and window controls.
#[cfg(target_os = "windows")]
fn frame_edge(handle: *mut std::ffi::c_void, lparam: isize) -> Option<isize> {
    use windows_sys::Win32::Foundation::{POINT, RECT};
    use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
    use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetClientRect, HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT,
        HTTOPRIGHT, SM_CXPADDEDBORDER, SM_CXSIZEFRAME, SM_CYSIZEFRAME,
    };

    if !framed(handle) || zoomed(handle) {
        return None;
    }
    let mut point = POINT {
        x: (lparam & 0xFFFF) as u16 as i16 as i32,
        y: ((lparam >> 16) & 0xFFFF) as u16 as i16 as i32,
    };
    let mut client = RECT::default();
    unsafe {
        if ScreenToClient(handle, &mut point) == 0 || GetClientRect(handle, &mut client) == 0 {
            return None;
        }
        let dpi = GetDpiForWindow(handle);
        let edge_x = GetSystemMetricsForDpi(SM_CXSIZEFRAME, dpi)
            + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
        let edge_y = GetSystemMetricsForDpi(SM_CYSIZEFRAME, dpi)
            + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
        let (width, height) = (client.right, client.bottom);
        let (top, bottom, left, right) = (
            0 <= point.y && point.y < edge_y,
            height - edge_y <= point.y && point.y < height,
            0 <= point.x && point.x < edge_x,
            width - edge_x <= point.x && point.x < width,
        );
        Some(match (top, bottom, left, right) {
            (true, _, true, _) => HTTOPLEFT,
            (true, _, _, true) => HTTOPRIGHT,
            (true, _, _, _) => HTTOP,
            (_, true, true, _) => HTBOTTOMLEFT,
            (_, true, _, true) => HTBOTTOMRIGHT,
            (_, true, _, _) => HTBOTTOM,
            (_, _, true, _) => HTLEFT,
            (_, _, _, true) => HTRIGHT,
            _ => return None,
        } as isize)
    }
}

/// Widens the client area over the frame's left, right and bottom edges, and serves the
/// resize hit tests those edges used to get from the frame. A maximized window keeps its
/// insets, which the screen's edge and the taskbar cover anyway.
#[cfg(target_os = "windows")]
unsafe extern "system" fn client_area(
    handle: *mut std::ffi::c_void,
    message: u32,
    wparam: usize,
    lparam: isize,
    _subclass: usize,
    data: usize,
) -> isize {
    use windows_sys::Win32::UI::Shell::DefSubclassProc;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        NCCALCSIZE_PARAMS, WM_NCCALCSIZE, WM_NCHITTEST,
    };

    let resizable = data != 0;
    unsafe {
        if message == WM_NCCALCSIZE
            && wparam != 0
            && (!resizable || (framed(handle) && !zoomed(handle)))
        {
            let proposed = (*(lparam as *const NCCALCSIZE_PARAMS)).rgrc[0];
            let result = DefSubclassProc(handle, message, wparam, lparam);
            let client = &mut (*(lparam as *mut NCCALCSIZE_PARAMS)).rgrc[0];
            client.left = proposed.left;
            client.right = proposed.right;
            client.bottom = proposed.bottom;
            return result;
        }
        if resizable
            && message == WM_NCHITTEST
            && let Some(edge) = frame_edge(handle, lparam)
        {
            return edge;
        }
        DefSubclassProc(handle, message, wparam, lparam)
    }
}