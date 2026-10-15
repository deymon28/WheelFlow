#![allow(non_snake_case, dead_code)]
use std::ffi::c_void;
pub type Handle = isize;
pub type HookProc = unsafe extern "system" fn(i32, usize, isize) -> isize;

#[repr(C)]
#[derive(Default, Clone, Copy, Debug)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}
#[repr(C)]
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}
#[repr(C)]
#[derive(Default)]
pub struct MonitorInfo {
    pub size: u32,
    pub monitor: Rect,
    pub work: Rect,
    pub flags: u32,
}
#[repr(C)]
#[derive(Default)]
pub struct MonitorInfoEx {
    pub info: MonitorInfo,
    pub device: [u16; 32],
}
#[repr(C)]
pub struct OpenAdapter {
    pub device: [u16; 32],
    pub adapter: u32,
    pub luid_low: u32,
    pub luid_high: i32,
    pub source: u32,
}
#[repr(C)]
pub struct WaitForVerticalBlank {
    pub adapter: u32,
    pub device: u32,
    pub source: u32,
}
#[repr(C)]
pub struct ShellExecuteInfo {
    pub size: u32,
    pub mask: u32,
    pub window: Handle,
    pub verb: *const u16,
    pub file: *const u16,
    pub parameters: *const u16,
    pub directory: *const u16,
    pub show: i32,
    pub instance: Handle,
    pub id_list: *mut c_void,
    pub class: *const u16,
    pub class_key: Handle,
    pub hot_key: u32,
    pub icon: Handle,
    pub process: Handle,
}
#[repr(C)]
pub struct PowerThrottling {
    pub version: u32,
    pub control_mask: u32,
    pub state_mask: u32,
}
#[repr(C)]
#[derive(Default)]
pub struct Msg {
    pub hwnd: Handle,
    pub message: u32,
    pub wparam: usize,
    pub lparam: isize,
    pub time: u32,
    pub point: Point,
    pub private: u32,
}
#[repr(C)]
pub struct MouseHook {
    pub point: Point,
    pub data: u32,
    pub flags: u32,
    pub time: u32,
    pub extra: usize,
}
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct MouseInput {
    pub dx: i32,
    pub dy: i32,
    pub data: u32,
    pub flags: u32,
    pub time: u32,
    pub extra: usize,
}
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct Input {
    pub kind: u32,
    pub mouse: MouseInput,
}

pub const WHEEL: usize = 0x020A;
pub const HWHEEL: usize = 0x020E;
pub const MOUSEMOVE: usize = 0x0200;
pub const WM_QUIT: u32 = 0x0012;
pub const WM_TIMER: u32 = 0x0113;
pub const WM_REHOOK: u32 = 0x8001;
pub const OWN_MARKER: usize = 0x57464C57;
pub const TEST_RAW: usize = 0x57465452;
pub const TEST_OUTPUT: usize = 0x5746544F;
pub const INFINITE: u32 = 0xFFFFFFFF;

pub fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
pub fn wheel_input(delta: i32, horizontal: bool, marker: usize) -> Input {
    Input {
        kind: 0,
        mouse: MouseInput {
            data: delta as u32,
            flags: if horizontal { 0x1000 } else { 0x0800 },
            extra: marker,
            ..Default::default()
        },
    }
}
pub fn root_at(point: Point) -> Handle {
    unsafe {
        let window = WindowFromPhysicalPoint(point);
        if window == 0 {
            0
        } else {
            GetAncestor(window, 2)
        }
    }
}
pub fn pointer_root() -> Handle {
    let mut point = Point::default();
    unsafe {
        if GetPhysicalCursorPos(&mut point) == 0 {
            0
        } else {
            root_at(point)
        }
    }
}
pub fn modifiers() -> bool {
    unsafe {
        [0x10, 0x11, 0x12, 0x5B, 0x5C]
            .iter()
            .any(|key| GetAsyncKeyState(*key) < 0)
    }
}
pub fn process_id(window: Handle) -> u32 {
    let mut pid = 0;
    unsafe {
        GetWindowThreadProcessId(window, &mut pid);
    }
    pid
}
/// A captionless top-level window covering its whole monitor: a fullscreen
/// game, video player, or presentation. Maximized windows keep their caption.
pub fn covers_monitor(window: Handle) -> bool {
    unsafe {
        let style = GetWindowLongPtrW(window, -16) as u32;
        // Captioned or maximized windows (including custom-frame apps with an
        // auto-hidden taskbar) are ordinary documents.
        if style & 0x00C00000 == 0x00C00000 || IsZoomed(window) != 0 {
            return false;
        }
        let mut rect = Rect::default();
        let mut info = MonitorInfo {
            size: std::mem::size_of::<MonitorInfo>() as u32,
            ..Default::default()
        };
        let monitor = MonitorFromWindow(window, 0);
        if monitor == 0
            || GetWindowRect(window, &mut rect) == 0
            || GetMonitorInfoW(monitor, &mut info) == 0
        {
            return false;
        }
        let m = info.monitor;
        rect.left <= m.left && rect.top <= m.top && rect.right >= m.right && rect.bottom >= m.bottom
    }
}
/// True while an application confines the pointer, as games do during play.
pub fn cursor_confined() -> bool {
    unsafe {
        let mut clip = Rect::default();
        if GetClipCursor(&mut clip) == 0 {
            return false;
        }
        let left = GetSystemMetrics(76);
        let top = GetSystemMetrics(77);
        let screen = Rect {
            left,
            top,
            right: left + GetSystemMetrics(78),
            bottom: top + GetSystemMetrics(79),
        };
        clip.left > screen.left
            || clip.top > screen.top
            || clip.right < screen.right
            || clip.bottom < screen.bottom
    }
}
/// Background processes may otherwise get EcoQoS and coarse timers on
/// Windows 11, which turns smooth output into irregular bursts.
pub fn opt_out_of_power_throttling() {
    let execution_speed = 0x1;
    let ignore_timer_resolution = 0x4;
    for mask in [execution_speed | ignore_timer_resolution, execution_speed] {
        let state = PowerThrottling {
            version: 1,
            control_mask: mask,
            state_mask: 0,
        };
        let applied = unsafe {
            SetProcessInformation(
                GetCurrentProcess(),
                4,
                (&state as *const PowerThrottling).cast(),
                std::mem::size_of::<PowerThrottling>() as u32,
            )
        };
        if applied != 0 {
            break;
        }
    }
}
pub fn elevated(process: Handle) -> Option<bool> {
    unsafe {
        let mut token = 0;
        if OpenProcessToken(process, 8, &mut token) == 0 {
            return None;
        }
        let mut flag: u32 = 0;
        let mut size = 0;
        let ok = GetTokenInformation(token, 20, (&mut flag as *mut u32).cast(), 4, &mut size);
        CloseHandle(token);
        if ok != 0 {
            Some(flag != 0)
        } else {
            None
        }
    }
}
pub fn running_elevated() -> bool {
    elevated(unsafe { GetCurrentProcess() }).unwrap_or(false)
}
/// Whether the foreground window belongs to an elevated process. Windows does
/// not pass its input to the low-level hooks of a non-elevated process.
pub fn foreground_elevated() -> bool {
    unsafe {
        let window = GetForegroundWindow();
        if window == 0 {
            return false;
        }
        let process = OpenProcess(0x1000, 0, process_id(window));
        if process == 0 {
            return false;
        }
        let result = elevated(process).unwrap_or(false);
        CloseHandle(process);
        result
    }
}
pub fn executable_path() -> Option<String> {
    let mut path = vec![0u16; 32768];
    let length = unsafe { GetModuleFileNameW(0, path.as_mut_ptr(), path.len() as u32) } as usize;
    (length > 0 && length < path.len()).then(|| String::from_utf16_lossy(&path[..length]))
}
/// Starts this executable again with a UAC elevation prompt.
pub fn relaunch_elevated(arguments: &str) -> bool {
    let Some(executable) = executable_path() else {
        return false;
    };
    let path = wide(&executable);
    let verb = wide("runas");
    let parameters = wide(arguments);
    let mut info = ShellExecuteInfo {
        size: std::mem::size_of::<ShellExecuteInfo>() as u32,
        mask: 0,
        window: 0,
        verb: verb.as_ptr(),
        file: path.as_ptr(),
        parameters: parameters.as_ptr(),
        directory: std::ptr::null(),
        show: 1,
        instance: 0,
        id_list: std::ptr::null_mut(),
        class: std::ptr::null(),
        class_key: 0,
        hot_key: 0,
        icon: 0,
        process: 0,
    };
    unsafe { ShellExecuteExW(&mut info) != 0 }
}
pub fn monitors() -> Vec<Handle> {
    unsafe extern "system" fn collect(
        monitor: Handle,
        _: Handle,
        _: *mut Rect,
        list: isize,
    ) -> i32 {
        (*(list as *mut Vec<Handle>)).push(monitor);
        1
    }
    let mut list: Vec<Handle> = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            0,
            std::ptr::null(),
            collect,
            &mut list as *mut Vec<Handle> as isize,
        );
    }
    list
}
pub fn monitor_of(window: Handle) -> Handle {
    let monitor = unsafe { MonitorFromWindow(window, 0) };
    if monitor != 0 {
        monitor
    } else {
        unsafe { MonitorFromPoint(Point::default(), 1) }
    }
}
pub fn monitor_device(monitor: Handle) -> Option<[u16; 32]> {
    let mut info = MonitorInfoEx::default();
    info.info.size = std::mem::size_of::<MonitorInfoEx>() as u32;
    if unsafe { GetMonitorInfoW(monitor, (&mut info as *mut MonitorInfoEx).cast()) } == 0 {
        None
    } else {
        Some(info.device)
    }
}
/// Nominal refresh rate of a display device, in Hz.
pub fn refresh_rate(device: &[u16; 32]) -> Option<u32> {
    let mut mode = [0u8; 220];
    mode[68..70].copy_from_slice(&220u16.to_le_bytes());
    let ok = unsafe { EnumDisplaySettingsW(device.as_ptr(), u32::MAX, mode.as_mut_ptr()) };
    let rate = u32::from_le_bytes(mode[184..188].try_into().unwrap());
    (ok != 0 && rate > 1).then_some(rate)
}
pub fn wheel_scroll_lines() -> Option<u32> {
    let mut lines = 0u32;
    let ok = unsafe { SystemParametersInfoW(0x68, 0, (&mut lines as *mut u32).cast(), 0) };
    (ok != 0).then_some(lines)
}
/// Changes the session value only; the user's saved preference is untouched.
pub fn set_wheel_scroll_lines(lines: u32) -> bool {
    unsafe { SystemParametersInfoW(0x69, lines, std::ptr::null_mut(), 2) != 0 }
}
/// The user's saved WheelScrollLines preference.
pub fn saved_wheel_scroll_lines() -> Option<u32> {
    get_user_value("Control Panel\\Desktop", "WheelScrollLines")?
        .trim()
        .parse()
        .ok()
}
const CURRENT_USER: Handle = 0x80000001u32 as i32 as isize;
/// Reads a string value under HKEY_CURRENT_USER.
pub fn get_user_value(subkey: &str, name: &str) -> Option<String> {
    let mut buffer = vec![0u16; 1024];
    let mut size = (buffer.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            CURRENT_USER,
            wide(subkey).as_ptr(),
            wide(name).as_ptr(),
            0x2, // RRF_RT_REG_SZ
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != 0 {
        return None;
    }
    buffer.truncate((size as usize / 2).saturating_sub(1));
    Some(String::from_utf16_lossy(&buffer))
}
/// Writes a string value under HKEY_CURRENT_USER, creating the key if needed.
pub fn set_user_value(subkey: &str, name: &str, value: &str) -> bool {
    let data = wide(value);
    unsafe {
        RegSetKeyValueW(
            CURRENT_USER,
            wide(subkey).as_ptr(),
            wide(name).as_ptr(),
            1, // REG_SZ
            data.as_ptr().cast(),
            (data.len() * 2) as u32,
        ) == 0
    }
}
/// Removes a value under HKEY_CURRENT_USER; a missing value is not an error.
pub fn delete_user_value(subkey: &str, name: &str) {
    unsafe {
        RegDeleteKeyValueW(CURRENT_USER, wide(subkey).as_ptr(), wide(name).as_ptr());
    }
}
pub fn set_thread_priority(priority: i32) {
    unsafe {
        SetThreadPriority(GetCurrentThread(), priority);
    }
}
pub fn class_name(window: Handle) -> String {
    let mut buffer = [0u16; 128];
    let length = unsafe { GetClassNameW(window, buffer.as_mut_ptr(), buffer.len() as i32) };
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

#[link(name = "user32")]
extern "system" {
    pub fn SetWindowsHookExW(
        kind: i32,
        callback: Option<HookProc>,
        module: Handle,
        thread: u32,
    ) -> Handle;
    pub fn UnhookWindowsHookEx(hook: Handle) -> i32;
    pub fn CallNextHookEx(hook: Handle, code: i32, message: usize, data: isize) -> isize;
    pub fn GetMessageW(message: *mut Msg, window: Handle, min: u32, max: u32) -> i32;
    pub fn PeekMessageW(message: *mut Msg, window: Handle, min: u32, max: u32, remove: u32) -> i32;
    pub fn TranslateMessage(message: *const Msg) -> i32;
    pub fn DispatchMessageW(message: *const Msg) -> isize;
    pub fn PostThreadMessageW(thread: u32, message: u32, wparam: usize, lparam: isize) -> i32;
    pub fn SendInput(count: u32, inputs: *const Input, size: i32) -> u32;
    pub fn GetAsyncKeyState(key: i32) -> i16;
    pub fn WindowFromPhysicalPoint(point: Point) -> Handle;
    pub fn GetPhysicalCursorPos(point: *mut Point) -> i32;
    pub fn GetAncestor(window: Handle, flags: u32) -> Handle;
    pub fn GetForegroundWindow() -> Handle;
    pub fn FindWindowW(class: *const u16, title: *const u16) -> Handle;
    pub fn PostMessageW(window: Handle, message: u32, wparam: usize, lparam: isize) -> i32;
    pub fn GetWindowThreadProcessId(window: Handle, pid: *mut u32) -> u32;
    pub fn SetProcessDpiAwarenessContext(context: isize) -> i32;
    pub fn SetThreadDpiAwarenessContext(context: isize) -> isize;
    pub fn MessageBoxW(window: Handle, text: *const u16, caption: *const u16, kind: u32) -> i32;
    pub fn GetWindowRect(window: Handle, rect: *mut Rect) -> i32;
    pub fn IsZoomed(window: Handle) -> i32;
    pub fn GetWindowLongPtrW(window: Handle, index: i32) -> isize;
    pub fn MonitorFromWindow(window: Handle, flags: u32) -> Handle;
    pub fn GetMonitorInfoW(monitor: Handle, info: *mut MonitorInfo) -> i32;
    pub fn GetClipCursor(rect: *mut Rect) -> i32;
    pub fn GetSystemMetrics(index: i32) -> i32;
    pub fn GetClassNameW(window: Handle, name: *mut u16, max_count: i32) -> i32;
    pub fn SetTimer(window: Handle, id: usize, milliseconds: u32, callback: *const c_void)
        -> usize;
    pub fn MonitorFromPoint(point: Point, flags: u32) -> Handle;
    pub fn AllowSetForegroundWindow(process: u32) -> i32;
    pub fn EnumDisplayMonitors(
        dc: Handle,
        clip: *const Rect,
        callback: unsafe extern "system" fn(Handle, Handle, *mut Rect, isize) -> i32,
        data: isize,
    ) -> i32;
    pub fn EnumDisplaySettingsW(device: *const u16, mode: u32, settings: *mut u8) -> i32;
    pub fn SystemParametersInfoW(action: u32, param: u32, value: *mut c_void, ini: u32) -> i32;
    pub fn ChangeWindowMessageFilterEx(
        window: Handle,
        message: u32,
        action: u32,
        status: *mut c_void,
    ) -> i32;
}
#[link(name = "gdi32")]
extern "system" {
    pub fn D3DKMTOpenAdapterFromGdiDisplayName(open: *mut OpenAdapter) -> i32;
    pub fn D3DKMTWaitForVerticalBlankEvent(wait: *const WaitForVerticalBlank) -> i32;
    pub fn D3DKMTCloseAdapter(adapter: *const u32) -> i32;
}
#[link(name = "shell32")]
extern "system" {
    pub fn ShellExecuteExW(info: *mut ShellExecuteInfo) -> i32;
}
#[link(name = "kernel32")]
extern "system" {
    pub fn GetModuleHandleW(name: *const u16) -> Handle;
    pub fn GetLastError() -> u32;
    pub fn SetLastError(error: u32);
    pub fn GetCurrentThreadId() -> u32;
    pub fn GetCurrentProcess() -> Handle;
    pub fn GetCurrentThread() -> Handle;
    pub fn SetThreadPriority(thread: Handle, priority: i32) -> i32;
    pub fn SetProcessInformation(
        process: Handle,
        class: i32,
        information: *const c_void,
        size: u32,
    ) -> i32;
    pub fn GetModuleFileNameW(module: Handle, name: *mut u16, size: u32) -> u32;
    pub fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    pub fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
    pub fn QueryFullProcessImageNameW(
        process: Handle,
        flags: u32,
        name: *mut u16,
        size: *mut u32,
    ) -> i32;
    pub fn CloseHandle(handle: Handle) -> i32;
    pub fn CreateMutexW(security: *const c_void, owner: i32, name: *const u16) -> Handle;
    pub fn CreateEventW(
        security: *const c_void,
        manual: i32,
        initial: i32,
        name: *const u16,
    ) -> Handle;
    pub fn SetEvent(event: Handle) -> i32;
    pub fn CreateWaitableTimerExW(
        security: *const c_void,
        name: *const u16,
        flags: u32,
        access: u32,
    ) -> Handle;
    pub fn SetWaitableTimer(
        timer: Handle,
        due: *const i64,
        period: i32,
        callback: *const c_void,
        arg: *const c_void,
        resume: i32,
    ) -> i32;
    pub fn CancelWaitableTimer(timer: Handle) -> i32;
    pub fn WaitForMultipleObjects(
        count: u32,
        handles: *const Handle,
        all: i32,
        milliseconds: u32,
    ) -> u32;
}
#[link(name = "advapi32")]
extern "system" {
    pub fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
    pub fn GetTokenInformation(
        token: Handle,
        class: u32,
        info: *mut c_void,
        size: u32,
        returned: *mut u32,
    ) -> i32;
    pub fn RegSetKeyValueW(
        key: Handle,
        subkey: *const u16,
        value: *const u16,
        kind: u32,
        data: *const c_void,
        size: u32,
    ) -> i32;
    pub fn RegDeleteKeyValueW(key: Handle, subkey: *const u16, value: *const u16) -> i32;
    pub fn RegDeleteTreeW(key: Handle, subkey: *const u16) -> i32;
    pub fn RegGetValueW(
        key: Handle,
        subkey: *const u16,
        value: *const u16,
        flags: u32,
        kind: *mut u32,
        data: *mut c_void,
        size: *mut u32,
    ) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_abi_layout_matches_windows_x64() {
        assert_eq!(std::mem::size_of::<Input>(), 40);
        assert_eq!(std::mem::offset_of!(Input, mouse), 8);
        assert_eq!(std::mem::size_of::<MouseInput>(), 32);
        assert_eq!(std::mem::size_of::<MouseHook>(), 32);
        assert_eq!(std::mem::size_of::<Msg>(), 48);
        assert_eq!(std::mem::size_of::<MonitorInfo>(), 40);
        assert_eq!(std::mem::size_of::<PowerThrottling>(), 12);
        assert_eq!(std::mem::size_of::<MonitorInfoEx>(), 104);
        assert_eq!(std::mem::size_of::<OpenAdapter>(), 80);
        assert_eq!(std::mem::size_of::<WaitForVerticalBlank>(), 12);
        assert_eq!(std::mem::size_of::<ShellExecuteInfo>(), 112);
    }

    #[test]
    fn reads_session_and_saved_wheel_lines_without_changing_them() {
        let session = wheel_scroll_lines().expect("session wheel lines");
        assert!(session >= 1);
        assert_eq!(wheel_scroll_lines(), Some(session));
        // A fresh profile (for example on a build server) may not store the value.
        if let Some(saved) = saved_wheel_scroll_lines() {
            assert!(saved >= 1);
        }
    }

    #[test]
    fn user_values_round_trip_and_delete() {
        let key = format!("Software\\WheelFlow.Test.{}", std::process::id());
        let command = r#""C:\Tools\WheelFlow.exe" --autostart"#;
        assert!(set_user_value(&key, "Command", command));
        assert_eq!(get_user_value(&key, "Command").as_deref(), Some(command));
        delete_user_value(&key, "Command");
        assert_eq!(get_user_value(&key, "Command"), None);
        delete_user_value(&key, "Command");
        unsafe {
            RegDeleteTreeW(CURRENT_USER, wide(&key).as_ptr());
        }
        assert!(executable_path()
            .map(|p| p.to_lowercase().ends_with(".exe"))
            .unwrap_or(false));
    }

    #[test]
    fn monitors_report_their_refresh_rates() {
        let screens = monitors();
        // Headless sessions may have no display at all.
        if screens.is_empty() {
            return;
        }
        assert!(screens.contains(&monitor_of(0)));
        for monitor in screens {
            let device = monitor_device(monitor).expect("monitor device name");
            if let Some(hz) = refresh_rate(&device) {
                assert!(hz >= 24, "{hz}");
            }
        }
    }
}
