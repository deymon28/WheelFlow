use crate::config::{Config, DURATION, SPEED};
use crate::win::{self, GetLastError};
use std::{
    cell::RefCell,
    ffi::c_void,
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
    rc::Rc,
};

type Handle = *mut c_void;
type WindowProc = unsafe extern "system" fn(Handle, u32, usize, isize) -> isize;

#[repr(C)]
struct WindowClass {
    size: u32,
    style: u32,
    proc: Option<WindowProc>,
    class_extra: i32,
    window_extra: i32,
    instance: Handle,
    icon: Handle,
    cursor: Handle,
    background: Handle,
    menu_name: *const u16,
    class_name: *const u16,
    small_icon: Handle,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Point {
    x: i32,
    y: i32,
}
#[repr(C)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}
#[repr(C)]
struct Message {
    window: Handle,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    point: Point,
    private: u32,
}
#[repr(C)]
struct NotifyIconData {
    size: u32,
    window: Handle,
    id: u32,
    flags: u32,
    callback: u32,
    icon: Handle,
    tip: [u16; 128],
    state: u32,
    state_mask: u32,
    info: [u16; 256],
    version: u32,
    info_title: [u16; 64],
    info_flags: u32,
    guid: [u8; 16],
    balloon_icon: Handle,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn RegisterClassExW(class: *const WindowClass) -> u16;
    fn UnregisterClassW(name: *const u16, instance: Handle) -> i32;
    fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: Handle,
        menu: Handle,
        instance: Handle,
        param: *mut c_void,
    ) -> Handle;
    fn DefWindowProcW(window: Handle, message: u32, wparam: usize, lparam: isize) -> isize;
    fn DestroyWindow(window: Handle) -> i32;
    fn IsDialogMessageW(window: Handle, message: *mut Message) -> i32;
    fn PostQuitMessage(code: i32);
    fn SendMessageW(window: Handle, message: u32, wparam: usize, lparam: isize) -> isize;
    fn ShowWindow(window: Handle, command: i32) -> i32;
    fn SetForegroundWindow(window: Handle) -> i32;
    fn SetFocus(window: Handle) -> Handle;
    fn LoadIconW(instance: Handle, name: *const u16) -> Handle;
    fn LoadCursorW(instance: Handle, name: *const u16) -> Handle;
    fn RegisterWindowMessageW(name: *const u16) -> u32;
    fn CreatePopupMenu() -> Handle;
    fn AppendMenuW(menu: Handle, flags: u32, id: usize, text: *const u16) -> i32;
    fn TrackPopupMenu(
        menu: Handle,
        flags: u32,
        x: i32,
        y: i32,
        reserved: i32,
        window: Handle,
        rect: *const Rect,
    ) -> i32;
    fn DestroyMenu(menu: Handle) -> i32;
    fn GetCursorPos(point: *mut Point) -> i32;
    fn GetWindowTextLengthW(window: Handle) -> i32;
    fn GetWindowTextW(window: Handle, text: *mut u16, max_count: i32) -> i32;
    fn GetDpiForWindow(window: Handle) -> u32;
    fn AdjustWindowRectExForDpi(
        rect: *mut Rect,
        style: u32,
        menu: i32,
        ex_style: u32,
        dpi: u32,
    ) -> i32;
    fn SetWindowPos(
        window: Handle,
        after: Handle,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        flags: u32,
    ) -> i32;
}
#[link(name = "shell32")]
unsafe extern "system" {
    fn Shell_NotifyIconW(message: u32, data: *const NotifyIconData) -> i32;
}
#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateFontW(
        height: i32,
        width: i32,
        escapement: i32,
        orientation: i32,
        weight: i32,
        italic: u32,
        underline: u32,
        strikeout: u32,
        charset: u32,
        out_precision: u32,
        clip_precision: u32,
        quality: u32,
        pitch: u32,
        face: *const u16,
    ) -> Handle;
    fn DeleteObject(object: Handle) -> i32;
}

const TRAY_MESSAGE: u32 = 0x8001;
const ENABLED: usize = 101;
const SETTINGS: usize = 102;
const DIAGNOSTICS: usize = 103;
const EXIT: usize = 104;
const SAVE: usize = 1;
const CANCEL: usize = 2;
const SETTINGS_STYLE: u32 = 0x00C80000; // Caption, system menu; fixed size.

struct State {
    owner: Handle,
    settings_window: Handle,
    speed: Handle,
    duration: Handle,
    acceleration: Handle,
    fullscreen: Handle,
    administrator: Handle,
    exclusions: Handle,
    font: Handle,
    controls: Vec<Handle>,
    config: Config,
    update: Rc<dyn Fn(Config)>,
    diagnostics: Rc<dyn Fn() -> String>,
    restart_elevated: Rc<dyn Fn() -> bool>,
    taskbar_created: u32,
}
thread_local! { static STATE: RefCell<Option<State>> = const { RefCell::new(None) }; }

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

// The UI uses pointer handles; the shared input bindings use pointer-sized integers.
// Keep one extern declaration for each imported Windows symbol.
unsafe fn module_handle() -> Handle {
    unsafe { win::GetModuleHandleW(null()) as Handle }
}

fn message_box(window: Handle, text: &str, error: bool) {
    unsafe {
        win::MessageBoxW(
            window as win::Handle,
            wide(text).as_ptr(),
            wide("WheelFlow").as_ptr(),
            if error { 0x10 } else { 0x40 },
        );
    }
}

fn icon_data(window: Handle) -> NotifyIconData {
    let mut data: NotifyIconData = unsafe { zeroed() };
    data.size = size_of::<NotifyIconData>() as u32;
    data.window = window;
    data.id = 1;
    data.flags = 1 | 2 | 4;
    data.callback = TRAY_MESSAGE;
    data.icon = unsafe { LoadIconW(null_mut(), 32512usize as *const u16) };
    let enabled = STATE.with(|s| {
        s.borrow()
            .as_ref()
            .map(|s| s.config.enabled)
            .unwrap_or(true)
    });
    let tip = wide(if enabled {
        "WheelFlow - enabled"
    } else {
        "WheelFlow - paused"
    });
    data.tip[..tip.len()].copy_from_slice(&tip);
    data
}

fn publish_config(config: Config) {
    let (update, owner) = STATE.with(|s| {
        let mut state = s.borrow_mut();
        let state = state.as_mut().unwrap();
        state.config = config.clone();
        (state.update.clone(), state.owner)
    });
    update(config.clone());
    if let Err(error) = config.save() {
        message_box(owner, &error, true);
    }
    unsafe {
        Shell_NotifyIconW(1, &icon_data(owner));
    }
}

fn action(id: usize) {
    match id {
        ENABLED => {
            let mut config = STATE.with(|s| s.borrow().as_ref().unwrap().config.clone());
            config.enabled = !config.enabled;
            publish_config(config);
        }
        SETTINGS => {
            if let Err(error) = show_settings() {
                message_box(null_mut(), &error, true);
            }
        }
        DIAGNOSTICS => {
            let (callback, owner) = STATE.with(|s| {
                let state = s.borrow();
                let state = state.as_ref().unwrap();
                (state.diagnostics.clone(), state.owner)
            });
            message_box(owner, &callback(), false);
        }
        EXIT => {
            let owner = STATE.with(|s| s.borrow().as_ref().unwrap().owner);
            unsafe {
                win::PostMessageW(owner as win::Handle, 0x10, 0, 0);
            }
        }
        _ => (),
    }
}

fn tray_menu(window: Handle) {
    let enabled = STATE.with(|s| s.borrow().as_ref().unwrap().config.enabled);
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return;
        }
        AppendMenuW(
            menu,
            if enabled { 8 } else { 0 },
            ENABLED,
            wide("Enabled").as_ptr(),
        );
        AppendMenuW(menu, 0, SETTINGS, wide("Settings...").as_ptr());
        AppendMenuW(menu, 0, DIAGNOSTICS, wide("Diagnostics...").as_ptr());
        AppendMenuW(menu, 0x800, 0, null());
        AppendMenuW(menu, 0, EXIT, wide("Exit").as_ptr());
        let mut point = Point::default();
        GetCursorPos(&mut point);
        SetForegroundWindow(window);
        let command = TrackPopupMenu(menu, 0x100 | 0x80 | 2, point.x, point.y, 0, window, null());
        DestroyMenu(menu);
        win::PostMessageW(window as win::Handle, 0, 0, 0);
        if command > 0 {
            action(command as usize);
        }
    }
}

unsafe extern "system" fn owner_proc(
    window: Handle,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    let taskbar = STATE.with(|s| s.borrow().as_ref().map(|s| s.taskbar_created).unwrap_or(0));
    if taskbar != 0 && message == taskbar {
        unsafe {
            Shell_NotifyIconW(0, &icon_data(window));
        }
        return 0;
    }
    match message {
        TRAY_MESSAGE => {
            match lparam as u32 {
                0x205 | 0x7B => tray_menu(window),
                0x203 => action(SETTINGS),
                _ => (),
            }
            0
        }
        0x10 => {
            let settings = STATE.with(|s| {
                s.borrow()
                    .as_ref()
                    .map(|s| s.settings_window)
                    .unwrap_or(null_mut())
            });
            unsafe {
                if !settings.is_null() {
                    DestroyWindow(settings);
                }
                Shell_NotifyIconW(2, &icon_data(window));
                DestroyWindow(window);
            }
            0
        }
        0x2 => {
            unsafe {
                PostQuitMessage(0);
            }
            0
        }
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

fn control_text(window: Handle) -> String {
    unsafe {
        let mut text = vec![0u16; (GetWindowTextLengthW(window).max(0) + 1) as usize];
        let length = GetWindowTextW(window, text.as_mut_ptr(), text.len() as i32);
        String::from_utf16_lossy(&text[..length.max(0) as usize])
    }
}

fn checked(control: Handle) -> bool {
    unsafe { SendMessageW(control, 0xF0, 0, 0) == 1 }
}

fn save_settings(window: Handle) {
    let (speed, duration, acceleration, fullscreen, administrator, exclusions, mut config) = STATE
        .with(|s| {
            let state = s.borrow();
            let state = state.as_ref().unwrap();
            (
                state.speed,
                state.duration,
                state.acceleration,
                state.fullscreen,
                state.administrator,
                state.exclusions,
                state.config.clone(),
            )
        });
    let speed_value = control_text(speed).trim().parse::<u32>();
    let duration_value = control_text(duration).trim().parse::<u32>();
    match (speed_value, duration_value) {
        (Ok(speed), Ok(duration)) if SPEED.contains(&speed) && DURATION.contains(&duration) => {
            config.speed = speed;
            config.duration = duration;
            config.acceleration = checked(acceleration);
            config.fullscreen_bypass = checked(fullscreen);
            let elevate =
                checked(administrator) && !config.run_as_admin && !win::running_elevated();
            config.run_as_admin = checked(administrator);
            config.exclusions = Config::exclusion_names(&control_text(exclusions));
            publish_config(config);
            unsafe {
                DestroyWindow(window);
            }
            if elevate {
                restart_elevated();
            }
        }
        _ => message_box(
            window,
            &format!(
                "Enter a scroll speed from {} to {} and a duration from {} to {} milliseconds.",
                SPEED.start(),
                SPEED.end(),
                DURATION.start(),
                DURATION.end()
            ),
            true,
        ),
    }
}

unsafe extern "system" fn settings_proc(
    window: Handle,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    match message {
        0x111 => {
            match wparam & 0xffff {
                SAVE => save_settings(window),
                CANCEL => unsafe {
                    DestroyWindow(window);
                },
                _ => (),
            }
            0
        }
        0x10 => {
            unsafe {
                DestroyWindow(window);
            }
            0
        }
        0x82 => {
            let font = STATE.with(|s| {
                let mut state = s.borrow_mut();
                if let Some(state) = state.as_mut() {
                    state.settings_window = null_mut();
                    state.controls.clear();
                    let font = state.font;
                    state.font = null_mut();
                    font
                } else {
                    null_mut()
                }
            });
            if !font.is_null() {
                unsafe {
                    DeleteObject(font);
                }
            }
            unsafe { DefWindowProcW(window, message, wparam, lparam) }
        }
        0x2E0 => {
            let rect = unsafe { &*(lparam as *const Rect) };
            unsafe {
                SetWindowPos(
                    window,
                    null_mut(),
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    0x14,
                );
            }
            layout_settings(window);
            0
        }
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

fn layout_settings(window: Handle) {
    let dpi = unsafe { GetDpiForWindow(window) }.max(96);
    let scale = |n: i32| ((n as i64 * dpi as i64 + 48) / 96) as i32;
    let font = unsafe {
        CreateFontW(
            -scale(13),
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            1,
            0,
            0,
            5,
            0,
            wide("Segoe UI").as_ptr(),
        )
    };
    let (controls, old_font) = STATE.with(|s| {
        let mut state = s.borrow_mut();
        let state = state.as_mut().unwrap();
        let old = if font.is_null() {
            null_mut()
        } else {
            let old = state.font;
            state.font = font;
            old
        };
        (state.controls.clone(), old)
    });
    let positions = [
        (20, 20, 195, 24),
        (225, 18, 130, 26),
        (20, 60, 195, 24),
        (225, 58, 130, 26),
        (20, 98, 335, 24),
        (20, 126, 335, 24),
        (20, 154, 335, 40),
        (20, 200, 335, 40),
        (20, 242, 335, 110),
        (195, 368, 75, 30),
        (280, 368, 75, 30),
    ];
    for (control, (x, y, w, h)) in controls.into_iter().zip(positions) {
        unsafe {
            SetWindowPos(
                control,
                null_mut(),
                scale(x),
                scale(y),
                scale(w),
                scale(h),
                0x14,
            );
            if !font.is_null() {
                SendMessageW(control, 0x30, font as usize, 1);
            }
        }
    }
    if !old_font.is_null() {
        unsafe {
            DeleteObject(old_font);
        }
    }
}

fn show_settings() -> Result<(), String> {
    let (existing, owner, config) = STATE.with(|s| {
        let state = s.borrow();
        let state = state.as_ref().unwrap();
        (state.settings_window, state.owner, state.config.clone())
    });
    if !existing.is_null() {
        unsafe {
            ShowWindow(existing, 9);
            SetForegroundWindow(existing);
        }
        return Ok(());
    }
    unsafe {
        let instance = module_handle();
        let window = CreateWindowExW(
            0x10000,
            wide("WheelFlowSettings").as_ptr(),
            wide("WheelFlow settings").as_ptr(),
            SETTINGS_STYLE,
            i32::MIN,
            i32::MIN,
            400,
            400,
            owner,
            null_mut(),
            instance,
            null_mut(),
        );
        if window.is_null() {
            return Err(format!(
                "Could not create settings window (Windows error {}).",
                GetLastError()
            ));
        }
        STATE.with(|s| s.borrow_mut().as_mut().unwrap().settings_window = window);
        let dpi = GetDpiForWindow(window).max(96);
        let mut rect = Rect {
            left: 0,
            top: 0,
            right: (375 * dpi / 96) as i32,
            bottom: (418 * dpi / 96) as i32,
        };
        AdjustWindowRectExForDpi(&mut rect, SETTINGS_STYLE, 0, 0x10000, dpi);
        SetWindowPos(
            window,
            null_mut(),
            0,
            0,
            rect.right - rect.left,
            rect.bottom - rect.top,
            0x16,
        );
        let specifications = [
            ("STATIC", "Scroll speed (%)".to_owned(), 0, 0),
            (
                "EDIT",
                config.speed.to_string(),
                0x00810000 | 0x2000 | 0x80,
                201,
            ),
            ("STATIC", "Glide duration (ms)".to_owned(), 0, 0),
            (
                "EDIT",
                config.duration.to_string(),
                0x00810000 | 0x2000 | 0x80,
                202,
            ),
            (
                "BUTTON",
                "Accelerate fast wheel spins".to_owned(),
                0x10000 | 3,
                204,
            ),
            (
                "BUTTON",
                "Leave fullscreen games and players unsmoothed".to_owned(),
                0x10000 | 3,
                205,
            ),
            (
                "BUTTON",
                "Run as administrator (also smooth elevated windows, such as an admin terminal)"
                    .to_owned(),
                0x10000 | 3 | 0x2000,
                206,
            ),
            (
                "STATIC",
                "Excluded apps (one per line, e.g. game.exe)".to_owned(),
                0,
                0,
            ),
            (
                "EDIT",
                config.exclusions.join("\r\n"),
                0x00A10000 | 4 | 0x40 | 0x1000,
                203,
            ),
            ("BUTTON", "OK".to_owned(), 0x10000 | 1, SAVE),
            ("BUTTON", "Cancel".to_owned(), 0x10000, CANCEL),
        ];
        let mut controls = Vec::new();
        for (class, text, style, id) in specifications {
            let control = CreateWindowExW(
                0,
                wide(class).as_ptr(),
                wide(&text).as_ptr(),
                0x50000000 | style,
                0,
                0,
                0,
                0,
                window,
                id as Handle,
                instance,
                null_mut(),
            );
            if control.is_null() {
                DestroyWindow(window);
                return Err("Could not create a settings control.".to_owned());
            }
            controls.push(control);
        }
        STATE.with(|s| {
            let mut state = s.borrow_mut();
            let state = state.as_mut().unwrap();
            state.speed = controls[1];
            state.duration = controls[3];
            state.acceleration = controls[4];
            state.fullscreen = controls[5];
            state.administrator = controls[6];
            state.exclusions = controls[8];
            state.controls = controls;
        });
        for (control, value) in [
            (
                STATE.with(|s| s.borrow().as_ref().unwrap().acceleration),
                config.acceleration,
            ),
            (
                STATE.with(|s| s.borrow().as_ref().unwrap().fullscreen),
                config.fullscreen_bypass,
            ),
            (
                STATE.with(|s| s.borrow().as_ref().unwrap().administrator),
                config.run_as_admin,
            ),
        ] {
            SendMessageW(control, 0xF1, value as usize, 0);
        }
        layout_settings(window);
        ShowWindow(window, 5);
        SetForegroundWindow(window);
        let speed = STATE.with(|s| s.borrow().as_ref().unwrap().speed);
        SetFocus(speed);
    }
    Ok(())
}

/// Starts an elevated copy and closes this one; reports a declined prompt.
fn restart_elevated() {
    let (restart, owner) = STATE.with(|s| {
        let state = s.borrow();
        let state = state.as_ref().unwrap();
        (state.restart_elevated.clone(), state.owner)
    });
    if restart() {
        unsafe {
            win::PostMessageW(owner as win::Handle, 0x10, 0, 0);
        }
    } else {
        message_box(
            owner,
            "Administrator rights were not granted. WheelFlow keeps running without them; elevated windows receive ordinary scrolling.",
            false,
        );
    }
}

pub fn run(
    config: Config,
    update: Box<dyn Fn(Config)>,
    diagnostics: Box<dyn Fn() -> String>,
) -> Result<(), String> {
    unsafe {
        let instance = module_handle();
        let owner_name = wide("WheelFlowTray");
        let settings_name = wide("WheelFlowSettings");
        for (name, proc) in [
            (&owner_name, owner_proc as WindowProc),
            (&settings_name, settings_proc as WindowProc),
        ] {
            let class = WindowClass {
                size: size_of::<WindowClass>() as u32,
                style: 0,
                proc: Some(proc),
                class_extra: 0,
                window_extra: 0,
                instance,
                icon: LoadIconW(null_mut(), 32512usize as *const u16),
                cursor: LoadCursorW(null_mut(), 32512usize as *const u16),
                background: 16usize as Handle,
                menu_name: null(),
                class_name: name.as_ptr(),
                small_icon: null_mut(),
            };
            if RegisterClassExW(&class) == 0 {
                let error = GetLastError();
                UnregisterClassW(owner_name.as_ptr(), instance);
                return Err(format!(
                    "Could not register UI class (Windows error {error})."
                ));
            }
        }
        let owner = CreateWindowExW(
            0,
            owner_name.as_ptr(),
            wide("WheelFlow").as_ptr(),
            0,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            instance,
            null_mut(),
        );
        if owner.is_null() {
            let error = GetLastError();
            UnregisterClassW(settings_name.as_ptr(), instance);
            UnregisterClassW(owner_name.as_ptr(), instance);
            return Err(format!(
                "Could not create tray window (Windows error {error})."
            ));
        }
        STATE.with(|s| {
            *s.borrow_mut() = Some(State {
                owner,
                settings_window: null_mut(),
                speed: null_mut(),
                duration: null_mut(),
                acceleration: null_mut(),
                fullscreen: null_mut(),
                administrator: null_mut(),
                exclusions: null_mut(),
                font: null_mut(),
                controls: Vec::new(),
                config,
                update: Rc::from(update),
                diagnostics: Rc::from(diagnostics),
                restart_elevated: Rc::new(|| win::relaunch_elevated("--restarted")),
                taskbar_created: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
            })
        });
        // Allow `--stop` from a non-elevated shell to close an elevated instance.
        win::ChangeWindowMessageFilterEx(owner as win::Handle, 0x10, 1, null_mut());
        if Shell_NotifyIconW(0, &icon_data(owner)) == 0 {
            DestroyWindow(owner);
            STATE.with(|s| *s.borrow_mut() = None);
            UnregisterClassW(settings_name.as_ptr(), instance);
            UnregisterClassW(owner_name.as_ptr(), instance);
            return Err("Could not add the notification area icon.".to_owned());
        }
        let mut message: Message = zeroed();
        let result = loop {
            let status =
                win::GetMessageW((&mut message as *mut Message).cast::<win::Msg>(), 0, 0, 0);
            if status == 0 {
                break Ok(());
            }
            if status == -1 {
                break Err(format!(
                    "UI message loop failed (Windows error {}).",
                    GetLastError()
                ));
            }
            let settings = STATE.with(|s| s.borrow().as_ref().unwrap().settings_window);
            if settings.is_null() || IsDialogMessageW(settings, &mut message) == 0 {
                win::TranslateMessage((&message as *const Message).cast::<win::Msg>());
                win::DispatchMessageW((&message as *const Message).cast::<win::Msg>());
            }
        };
        if result.is_err() {
            SendMessageW(owner, 0x10, 0, 0);
        }
        STATE.with(|s| *s.borrow_mut() = None);
        UnregisterClassW(settings_name.as_ptr(), instance);
        UnregisterClassW(owner_name.as_ptr(), instance);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_x64_abi_layouts_match_sdk() {
        assert_eq!(size_of::<Handle>(), 8);
        assert_eq!(size_of::<WindowClass>(), 80);
        assert_eq!(size_of::<Message>(), 48);
        assert_eq!(size_of::<NotifyIconData>(), 976);
    }
}
