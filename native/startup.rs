//! Optional start with Windows. A normal instance uses the per-user Run key.
//! Administrator mode uses a scheduled logon task with the highest run level,
//! which starts WheelFlow elevated without a UAC prompt at every sign-in; only
//! an elevated instance can register or remove that task.
use crate::{config::Config, win};
use std::{os::windows::process::CommandExt, process::Command};

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const NAME: &str = "WheelFlow";
/// Passed by automatic starts, so a duplicate start exits quietly.
pub const ARGUMENT: &str = "--autostart";

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Task Scheduler definition: run at this user's sign-in with the highest
/// available rights, at normal priority, without a time limit or battery stop.
fn task_xml(executable: &str, user: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Starts WheelFlow smooth scrolling with administrator rights at sign-in.</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>4</Priority>
    <AllowStartOnDemand>true</AllowStartOnDemand>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>"{command}"</Command>
      <Arguments>{ARGUMENT}</Arguments>
    </Exec>
  </Actions>
</Task>
"#,
        user = escape_xml(user),
        command = escape_xml(executable),
    )
}

fn schtasks(arguments: &[&str]) -> bool {
    let system = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let program = std::path::Path::new(&system)
        .join("System32")
        .join("schtasks.exe");
    Command::new(program)
        .args(arguments)
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn task_exists() -> bool {
    schtasks(&["/Query", "/TN", NAME])
}

fn register_task(executable: &str) -> Result<(), String> {
    let user = match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
        (Ok(domain), Ok(name)) => format!("{domain}\\{name}"),
        (_, Ok(name)) => name,
        _ => return Err("The current user name is unavailable.".into()),
    };
    let path = std::env::temp_dir().join(format!("WheelFlow-task-{}.xml", std::process::id()));
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(
        task_xml(executable, &user)
            .encode_utf16()
            .flat_map(u16::to_le_bytes),
    );
    std::fs::write(&path, bytes).map_err(|e| format!("Could not prepare the startup task: {e}"))?;
    let created = schtasks(&[
        "/Create",
        "/F",
        "/TN",
        NAME,
        "/XML",
        &path.to_string_lossy(),
    ]);
    let _ = std::fs::remove_file(&path);
    if created {
        Ok(())
    } else {
        Err("Windows did not accept the administrator startup task.".into())
    }
}

/// Makes the startup registration match the preferences. Elevated work that
/// is impossible from a normal instance is completed by the next elevated start.
pub fn sync(config: &Config) -> Result<(), String> {
    let executable = win::executable_path().ok_or("The program location is unavailable.")?;
    let elevated = win::running_elevated();
    let wants_task = config.autostart && config.run_as_admin;
    if wants_task && elevated {
        register_task(&executable)?;
        win::delete_user_value(RUN_KEY, NAME);
        return Ok(());
    }
    if elevated && task_exists() {
        schtasks(&["/Delete", "/F", "/TN", NAME]);
    }
    if config.autostart {
        // Until an elevated instance registers the task, the Run key starts a
        // normal instance, which then asks for elevation itself.
        let command = format!("\"{executable}\" {ARGUMENT}");
        if !win::set_user_value(RUN_KEY, NAME, &command) {
            return Err("Could not register WheelFlow to start with Windows.".into());
        }
    } else {
        win::delete_user_value(RUN_KEY, NAME);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{escape_xml, task_xml};

    #[test]
    fn task_definition_escapes_paths_and_keeps_normal_priority() {
        let xml = task_xml(r"C:\Tools & More\WheelFlow.exe", r"PC\user<1>");
        assert!(xml.contains(r#"<Command>"C:\Tools &amp; More\WheelFlow.exe"</Command>"#));
        assert!(xml.contains("<UserId>PC\\user&lt;1&gt;</UserId>"));
        assert!(xml.contains("<RunLevel>HighestAvailable</RunLevel>"));
        assert!(xml.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        assert!(xml.contains("<Priority>4</Priority>"));
        assert!(xml.contains("<Arguments>--autostart</Arguments>"));
        assert_eq!(escape_xml("a\"b"), "a&quot;b");
    }
}
