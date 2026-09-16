//! What a start writes: the prefs, the extension, the shim, and the manifest that names it.
//!
//! All of it every start, and none of it twice: a file whose bytes are already right is left
//! alone, so Thunderbird's sideload cache sees a changed XPI only when the extension changed.

use super::{EXTENSION_ID, Paths};
use crate::error::Result;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const USER_JS: &str = include_str!("../../assets/user.js");
const SHIM: &str = include_str!("../../assets/nm-shim.py");
/// `bridge/`, zipped by `build.rs`.
const XPI: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/bridge.xpi"));

pub fn everything(paths: &Paths, dev: bool) -> Result<()> {
    std::fs::create_dir_all(paths.profile.join("extensions"))?;
    write_if_changed(&paths.profile.join("user.js"), user_js(dev).as_bytes())?;
    write_if_changed(&paths.profile.join("extensions").join(format!("{EXTENSION_ID}.xpi")), XPI)?;
    if let Some(dir) = paths.shim.parent() {
        std::fs::create_dir_all(dir)?;
    }
    write_if_changed(&paths.shim, SHIM.as_bytes())?;
    std::fs::set_permissions(&paths.shim, std::fs::Permissions::from_mode(0o755))?;
    if let Some(dir) = paths.manifest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Rewritten whenever it differs, which it does whenever the shim's path does.
    write_if_changed(&paths.manifest, manifest(&paths.shim).as_bytes())?;
    if let Some(dir) = paths.log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&paths.log, b"")?;
    Ok(())
}

/// The prefs, with the bridge's dev escape hatch on when asked for.
pub fn user_js(dev: bool) -> String {
    if dev {
        USER_JS.replace(
            "user_pref(\"extensions.noctmalia.dev\", false);",
            "user_pref(\"extensions.noctmalia.dev\", true);",
        )
    } else {
        USER_JS.to_string()
    }
}

/// The native-messaging host manifest. `allowed_extensions` is the whole of the trust decision:
/// only our extension may start the shim.
pub fn manifest(shim: &Path) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "name": "noctmalia.bridge",
        "description": "noctmalia's bridge to the Thunderbird it runs",
        "path": shim.display().to_string(),
        "type": "stdio",
        "allowed_extensions": [EXTENSION_ID],
    }))
    .expect("a literal serialises")
        + "\n"
}

fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<bool> {
    if std::fs::read(path).ok().as_deref() == Some(bytes) {
        return Ok(false);
    }
    std::fs::write(path, bytes)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn paths(name: &str) -> Paths {
        let root = std::env::temp_dir().join(format!("noctmalia-provision-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        Paths {
            install: root.join("install"),
            profile: root.join("profile"),
            shim: root.join("data/nm-shim.py"),
            manifest: root.join("mozilla/native-messaging-hosts/noctmalia.bridge.json"),
            cache: root.join("cache"),
            log: root.join("state/thunderbird.log"),
            socket: root.join("run/bridge.sock"),
        }
    }

    fn mtime(path: &Path) -> std::time::SystemTime {
        std::fs::metadata(path).unwrap().modified().unwrap()
    }

    #[test]
    fn everything_a_start_needs_is_written() {
        let paths = paths("all");
        everything(&paths, false).expect("provisions");
        assert!(paths.profile.join("user.js").is_file());
        let xpi = std::fs::read(paths.profile.join("extensions/bridge@noctmalia.xpi")).unwrap();
        assert_eq!(&xpi[..4], b"PK\x03\x04", "an XPI is a zip");
        assert!(std::fs::metadata(&paths.shim).unwrap().permissions().mode() & 0o111 != 0, "the shim is executable");
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&paths.manifest).unwrap()).unwrap();
        assert_eq!(manifest["path"], paths.shim.display().to_string());
        assert_eq!(manifest["allowed_extensions"], serde_json::json!([EXTENSION_ID]));
        assert!(paths.log.is_file(), "the log is started fresh");
    }

    #[test]
    fn an_unchanged_xpi_is_left_alone_and_a_changed_one_is_replaced() {
        let paths = paths("xpi");
        everything(&paths, false).unwrap();
        let xpi = paths.profile.join("extensions/bridge@noctmalia.xpi");
        let before = mtime(&xpi);
        std::thread::sleep(Duration::from_millis(20));
        everything(&paths, false).unwrap();
        assert_eq!(mtime(&xpi), before, "same bytes, same file");
        std::fs::write(&xpi, b"stale").unwrap();
        everything(&paths, false).unwrap();
        assert_eq!(std::fs::read(&xpi).unwrap(), XPI, "a stale XPI is replaced");
    }

    #[test]
    fn the_dev_flag_turns_the_dev_pref_on_and_only_that() {
        assert!(user_js(false).contains("user_pref(\"extensions.noctmalia.dev\", false);"));
        assert!(user_js(true).contains("user_pref(\"extensions.noctmalia.dev\", true);"));
        assert_eq!(user_js(true).lines().count(), user_js(false).lines().count());
    }

    #[test]
    fn the_manifest_follows_the_shim_wherever_it_moves() {
        let paths = paths("manifest");
        everything(&paths, false).unwrap();
        let moved = Paths { shim: paths.shim.with_file_name("elsewhere.py"), ..paths.clone() };
        everything(&moved, false).unwrap();
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&moved.manifest).unwrap()).unwrap();
        assert_eq!(manifest["path"], moved.shim.display().to_string());
    }
}
