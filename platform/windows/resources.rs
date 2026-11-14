//! What the Windows executable carries in its resources (plan stage 11, row P5; doc 07 §11.7, §13.1):
//! the icon, the version information and the application manifest that `tauri-build` 2.7.1 embedded in
//! today's Tauri `rustradar.exe`, written by `crates/app/build.rs` (which includes this file) as a
//! resource script for the Windows resource compiler. No dependencies, so that the build script stays a
//! plain one; `cargo test -p xtask` runs the tests below (`crates/xtask/src/package.rs` includes this
//! file too), and `cargo xtask package nsis msi` refuses an executable without these resources.
//!
//! Today's `rustradar.exe` (v0.3.16's, out of its NSIS installer) holds, all in language 1033:
//! - the icon group 32512 (`tauri_utils::platform::WINDOWS_APP_ICON_RESOURCE_ID`, the one Tauri's window
//!   loads) with the six images of `src-tauri/icons/icon.ico` (`bundle.icon`'s `.ico`);
//! - version information 1: FILEVERSION and PRODUCTVERSION `major, minor, patch, build`
//!   (`to_winres_version`: the build metadata if it is a number that fits 16 bits, else 0), and in the
//!   block `000004b0` (neutral language, Unicode) CompanyName `felix` (the identifier's second part, as
//!   tauri-build and both bundlers derive the publisher), FileDescription and ProductName `RustRadar`,
//!   FileVersion and ProductVersion the version as written; Translation `0x0, 0x04b0`;
//! - manifest 1: tauri-build's `windows-app-manifest.xml`, the dependency on Common Controls 6 only.
//!
//! What changes: the manifest also declares per-monitor-v2 DPI awareness (`dpiAwareness` `PerMonitorV2`
//! for Windows 10 1607 and later, `dpiAware` `true/pm` before; tao made Tauri's process DPI aware at run
//! time, as winit does when its event loop is made, and the manifest makes it so from the start, doc 07
//! §11.7) and the execution level: today's manifest names none, so Windows runs the 64-bit executable
//! as the invoker (no installer detection or virtualisation for 64-bit programs); `asInvoker` written
//! out is that same level, with no elevation prompt.

/// `productName` of `src-tauri/tauri.conf.json`: FileDescription and ProductName.
pub const PRODUCT_NAME: &str = "RustRadar";

/// The second part of the identifier `com.felix.rustradar` (no `bundle.publisher` is set): CompanyName.
pub const COMPANY_NAME: &str = "felix";

/// The icon group's id, tauri-utils' `WINDOWS_APP_ICON_RESOURCE_ID` (the app's window loads it).
pub const ICON_ID: u16 = 32512;

/// The application manifest (resource type 24, id 1: `CREATEPROCESS_MANIFEST_RESOURCE_ID`).
pub const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false" />
      </requestedPrivileges>
    </security>
  </trustInfo>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </windowsSettings>
  </application>
</assembly>
"#;

/// FILEVERSION and PRODUCTVERSION of `version` (`major.minor.patch[-pre][+build]`) as tauri-build's
/// `to_winres_version` packs them: the build metadata when it is a number of 16 bits, else 0; the
/// pre-release is left out. None for anything else, or a part beyond 16 bits.
pub fn version_numbers(version: &str) -> Option<[u16; 4]> {
    let (rest, build) = version.split_once('+').unwrap_or((version, ""));
    let core = rest.split_once('-').map_or(rest, |(core, _)| core);
    let parts: Vec<u16> = core.split('.').map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let [major, minor, patch] = parts[..] else { return None };
    Some([major, minor, patch, build.parse().unwrap_or(0)])
}

/// A string as the resource compiler reads it between double quotes.
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\"\""))
}

/// The resource script: the version information, the icon `icon` (an `.ico` file) and the manifest
/// `manifest` (a file holding [`MANIFEST`]), for the app at `version`.
pub fn resource_script(version: &str, icon: &str, manifest: &str) -> Result<String, String> {
    let [major, minor, patch, build] = version_numbers(version).ok_or_else(|| format!("`{version}` is not a version the Windows resources can carry"))?;
    let numbers = format!("{major}, {minor}, {patch}, {build}");
    let strings = [
        ("CompanyName", COMPANY_NAME),
        ("FileDescription", PRODUCT_NAME),
        ("FileVersion", version),
        ("ProductName", PRODUCT_NAME),
        ("ProductVersion", version),
    ];
    let mut script = String::from("#pragma code_page(65001)\n1 VERSIONINFO\n");
    script.push_str(&format!("FILEVERSION {numbers}\nPRODUCTVERSION {numbers}\n"));
    script.push_str("FILEFLAGSMASK 0x3f\nFILEFLAGS 0x0\nFILEOS 0x40004\nFILETYPE 0x1\nFILESUBTYPE 0x0\n");
    script.push_str("{\nBLOCK \"StringFileInfo\"\n{\nBLOCK \"000004b0\"\n{\n");
    for (name, value) in strings {
        script.push_str(&format!("VALUE {}, {}\n", quoted(name), quoted(value)));
    }
    script.push_str("}\n}\nBLOCK \"VarFileInfo\" {\nVALUE \"Translation\", 0x0, 0x04b0\n}\n}\n");
    script.push_str(&format!("{ICON_ID} ICON {}\n", quoted(icon)));
    script.push_str(&format!("1 24 {}\n", quoted(manifest)));
    Ok(script)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// tauri-build's own `to_winres_version` tests, as numbers.
    #[test]
    fn versions_are_packed_as_tauri_build_packs_them() {
        assert_eq!(version_numbers("0.3.16"), Some([0, 3, 16, 0]));
        assert_eq!(version_numbers("1.2.3+42"), Some([1, 2, 3, 42]));
        assert_eq!(version_numbers("1.2.3+42.sha"), Some([1, 2, 3, 0]));
        assert_eq!(version_numbers("1.2.3+abc"), Some([1, 2, 3, 0]));
        assert_eq!(version_numbers("1.2.3+70000"), Some([1, 2, 3, 0]));
        assert_eq!(version_numbers("1.2.3-beta.1+7"), Some([1, 2, 3, 7]));
        assert_eq!(version_numbers("1.2"), None);
        assert_eq!(version_numbers("1.2.x"), None);
        assert_eq!(version_numbers("70000.0.0"), None);
    }

    /// The version block and strings are v0.3.16's `rustradar.exe`'s, at that version.
    #[test]
    fn the_script_carries_todays_resources() {
        let script = resource_script("0.3.16", "C:\\a b\\icon.ico", "C:\\out\\rustradar.manifest").unwrap();
        let expected = "#pragma code_page(65001)\n1 VERSIONINFO\nFILEVERSION 0, 3, 16, 0\nPRODUCTVERSION 0, 3, 16, 0\n\
            FILEFLAGSMASK 0x3f\nFILEFLAGS 0x0\nFILEOS 0x40004\nFILETYPE 0x1\nFILESUBTYPE 0x0\n\
            {\nBLOCK \"StringFileInfo\"\n{\nBLOCK \"000004b0\"\n{\n\
            VALUE \"CompanyName\", \"felix\"\nVALUE \"FileDescription\", \"RustRadar\"\nVALUE \"FileVersion\", \"0.3.16\"\n\
            VALUE \"ProductName\", \"RustRadar\"\nVALUE \"ProductVersion\", \"0.3.16\"\n}\n}\n\
            BLOCK \"VarFileInfo\" {\nVALUE \"Translation\", 0x0, 0x04b0\n}\n}\n\
            32512 ICON \"C:\\\\a b\\\\icon.ico\"\n1 24 \"C:\\\\out\\\\rustradar.manifest\"\n";
        assert_eq!(script, expected);
        assert!(resource_script("next", "i.ico", "m").is_err());
        assert_eq!(quoted("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    /// tauri-build's manifest (the Common Controls 6 dependency, byte for byte in its element), and what
    /// is added: per-monitor-v2 DPI awareness and the invoker's execution level.
    #[test]
    fn the_manifest_keeps_tauri_builds_dependency() {
        let tauri_build = "<dependency>\n    <dependentAssembly>\n      <assemblyIdentity\n        type=\"win32\"\n        name=\"Microsoft.Windows.Common-Controls\"\n        version=\"6.0.0.0\"\n        processorArchitecture=\"*\"\n        publicKeyToken=\"6595b64144ccf1df\"\n        language=\"*\"\n      />\n    </dependentAssembly>\n  </dependency>";
        assert!(MANIFEST.contains(tauri_build));
        assert!(MANIFEST.contains(">PerMonitorV2</dpiAwareness>"));
        assert!(MANIFEST.contains(">true/pm</dpiAware>"));
        assert!(MANIFEST.contains("<requestedExecutionLevel level=\"asInvoker\" uiAccess=\"false\" />"));
        assert_eq!(MANIFEST.matches("<assembly ").count(), 1);
        assert!(MANIFEST.trim_end().ends_with("</assembly>"));
    }
}
