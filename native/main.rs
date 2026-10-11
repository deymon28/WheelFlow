#![cfg_attr(not(test), windows_subsystem = "windows")]
mod clock;
mod config;
mod engine;
mod runtime;
mod ui;
mod verify;
mod win;

fn main() {
    unsafe {
        win::SetProcessDpiAwarenessContext(-4);
    }
    win::opt_out_of_power_throttling();
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--stop") {
        unsafe {
            let window = win::FindWindowW(
                win::wide("WheelFlowTray").as_ptr(),
                win::wide("WheelFlow").as_ptr(),
            );
            if window != 0 {
                win::PostMessageW(window, 0x10, 0, 0);
            }
        }
        return;
    }
    if args.get(1).map(String::as_str) == Some("--verify") {
        let Some(path) = args.get(2) else {
            std::process::exit(2);
        };
        let result = verify::run();
        let text = match &result {
            Ok(report) => report.clone(),
            Err(error) => format!("FAIL: {}\n", error),
        };
        if std::fs::write(path, text).is_err() {
            std::process::exit(3);
        }
        if result.is_err() {
            std::process::exit(1);
        }
        return;
    }
    let restarted = args.iter().any(|a| a == "--restarted");
    let config = config::Config::load();
    // An administrator preference asks for elevation once per start; a
    // declined prompt continues without it.
    if config.run_as_admin
        && !restarted
        && !win::running_elevated()
        && win::relaunch_elevated("--restarted")
    {
        return;
    }
    // A restarted copy waits briefly for the previous instance to exit.
    let attempts = if restarted { 50 } else { 1 };
    let mut mutex = 0;
    for attempt in 0..attempts {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        mutex = unsafe {
            win::CreateMutexW(
                std::ptr::null(),
                0,
                win::wide("Local\\WheelFlow.Tray").as_ptr(),
            )
        };
        let error = unsafe { win::GetLastError() };
        // 183: exists; 5: an elevated instance owns it.
        if mutex != 0 && error != 183 {
            break;
        }
        if mutex != 0 {
            unsafe {
                win::CloseHandle(mutex);
            }
        }
        mutex = if error == 183 || error == 5 { -1 } else { 0 };
    }
    if mutex == 0 {
        show_error("Could not create the single-instance guard.");
        return;
    }
    if mutex == -1 {
        show_error("WheelFlow is already running. Open its tray icon.");
        return;
    }
    let result = run(config);
    unsafe {
        win::CloseHandle(mutex);
    }
    if let Err(error) = result {
        show_error(&error);
    }
}

fn run(config: config::Config) -> Result<(), String> {
    let mut runtime = runtime::Runtime::start(config.clone(), false)?;
    let update = runtime.shared.clone();
    let diagnostics = runtime.shared.clone();
    let result = ui::run(
        config,
        Box::new(move |config| update.update(config)),
        Box::new(move || diagnostics.diagnostics()),
    );
    runtime.stop();
    result
}

fn show_error(error: &str) {
    unsafe {
        win::MessageBoxW(
            0,
            win::wide(error).as_ptr(),
            win::wide("WheelFlow").as_ptr(),
            0x10,
        );
    }
}
