//! Generates the executable's Win32 resources (icon, version information, and
//! application manifest) as a compiled .res file, without external tools.
#[path = "native/icon.rs"]
#[allow(dead_code)]
mod icon;

use std::{env, fs, path::PathBuf};

const LANGUAGE: u16 = 0x0409;
const ICON_SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 128];

fn utf16z(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(Some(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn align(data: &mut Vec<u8>) {
    while data.len() % 4 != 0 {
        data.push(0);
    }
}

/// Appends one resource with ordinal type and name to a .res stream.
fn resource(out: &mut Vec<u8>, kind: u16, id: u16, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&32u32.to_le_bytes());
    for value in [0xFFFF, kind, 0xFFFF, id] {
        out.extend_from_slice(&u16::to_le_bytes(value));
    }
    out.extend_from_slice(&0u32.to_le_bytes()); // DataVersion
    out.extend_from_slice(&0x1030u16.to_le_bytes()); // Moveable, pure, discardable
    out.extend_from_slice(&LANGUAGE.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // Version
    out.extend_from_slice(&0u32.to_le_bytes()); // Characteristics
    out.extend_from_slice(data);
    align(out);
}

/// A VERSIONINFO block: header, key, optional value, then children.
fn block(key: &str, value: &[u8], value_length: u16, text: bool, children: &[Vec<u8>]) -> Vec<u8> {
    let mut data = vec![0u8; 6];
    data.extend_from_slice(&utf16z(key));
    align(&mut data);
    data.extend_from_slice(value);
    for child in children {
        align(&mut data);
        data.extend_from_slice(child);
    }
    let length = data.len() as u16;
    data[0..2].copy_from_slice(&length.to_le_bytes());
    data[2..4].copy_from_slice(&value_length.to_le_bytes());
    data[4..6].copy_from_slice(&(text as u16).to_le_bytes());
    data
}

fn string(key: &str, value: &str) -> Vec<u8> {
    let encoded = utf16z(value);
    block(key, &encoded, (encoded.len() / 2) as u16, true, &[])
}

fn version_info(version: &str) -> Vec<u8> {
    let parts: Vec<u32> = version
        .split('.')
        .map(|p| p.parse().unwrap_or(0))
        .chain(std::iter::repeat(0))
        .take(4)
        .collect();
    let most = (parts[0] << 16) | parts[1];
    let least = (parts[2] << 16) | parts[3];
    let mut fixed = Vec::new();
    for value in [
        0xFEEF04BDu32, // Signature
        0x0001_0000,   // Structure version
        most,
        least,
        most,
        least,
        0x3F,        // Flags mask
        0,           // Flags
        0x0004_0004, // VOS_NT_WINDOWS32
        1,           // VFT_APP
        0,
        0,
        0,
    ] {
        fixed.extend_from_slice(&value.to_le_bytes());
    }
    let strings = block(
        "040904b0",
        &[],
        0,
        true,
        &[
            string("FileDescription", "WheelFlow smooth mouse-wheel scrolling"),
            string("FileVersion", version),
            string("InternalName", "WheelFlow"),
            string("OriginalFilename", "WheelFlow.exe"),
            string("ProductName", "WheelFlow"),
            string("ProductVersion", version),
        ],
    );
    let translation = block("Translation", &0x04B0_0409u32.to_le_bytes(), 4, false, &[]);
    block(
        "VS_VERSION_INFO",
        &fixed,
        fixed.len() as u16,
        false,
        &[
            block("StringFileInfo", &[], 0, true, &[strings]),
            block("VarFileInfo", &[], 0, true, &[translation]),
        ],
    )
}

fn manifest(version: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="WheelFlow" version="{version}.0" processorArchitecture="amd64"/>
  <description>WheelFlow smooth mouse-wheel scrolling</description>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </windowsSettings>
  </application>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>
"#
    )
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=native/icon.rs");
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let mut res = Vec::new();
    // A .res stream starts with an empty resource entry.
    res.extend_from_slice(&0u32.to_le_bytes());
    res.extend_from_slice(&32u32.to_le_bytes());
    for value in [0xFFFFu16, 0, 0xFFFF, 0] {
        res.extend_from_slice(&value.to_le_bytes());
    }
    res.extend_from_slice(&[0u8; 16]);

    let mut group = Vec::new();
    for value in [0u16, 1, ICON_SIZES.len() as u16] {
        group.extend_from_slice(&value.to_le_bytes());
    }
    for (index, size) in ICON_SIZES.iter().enumerate() {
        let image = icon::icon_image(*size, icon::Look::Enabled);
        let id = index as u16 + 1;
        resource(&mut res, 3, id, &image); // RT_ICON
        group.extend_from_slice(&[*size as u8, *size as u8, 0, 0]);
        group.extend_from_slice(&1u16.to_le_bytes());
        group.extend_from_slice(&32u16.to_le_bytes());
        group.extend_from_slice(&(image.len() as u32).to_le_bytes());
        group.extend_from_slice(&id.to_le_bytes());
    }
    resource(&mut res, 14, 1, &group); // RT_GROUP_ICON
    resource(&mut res, 16, 1, &version_info(&version)); // RT_VERSION
    resource(&mut res, 24, 1, manifest(&version).as_bytes()); // RT_MANIFEST

    let path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("wheelflow.res");
    fs::write(&path, res).expect("write resources");
    println!("cargo:rustc-link-arg-bins={}", path.display());
}
