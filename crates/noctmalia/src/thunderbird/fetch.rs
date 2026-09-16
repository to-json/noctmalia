//! The pinned Mozilla build, fetched once.
//!
//! Through `curl` and `tar` rather than an HTTP client of our own: the mail pipeline's claim that
//! nothing in this program fetches from the network is about mail, and it stays literally true
//! of the binary — the one download it ever makes is a subprocess, at install, of a URL nobody
//! but Mozilla chooses.

use super::{Paths, VERSION};
use crate::error::{Error, Result};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

const URL: &str =
    "https://archive.mozilla.org/pub/thunderbird/releases/155.0.1/linux-x86_64/en-US/thunderbird-155.0.1.tar.xz";
/// From https://archive.mozilla.org/pub/thunderbird/releases/155.0.1/SHA512SUMS.
const SHA512: &str = "76625ec65ea09b723e3b0492ebb660e1e15d8efdcb3dbb5e6ea31fc22d6c7161bde6ddece2286751d0bf4263fc61ed3c76e5874706d15184d8b0c424a93549d9";
/// The tarball's size, for a progress bar that is a fraction rather than a number.
const BYTES: u64 = 86_238_724;

/// Thunderbird's own updater ships in the tarball and, outside a root-owned `/opt`, would use
/// it. Policies are what disable it for the parts a pref cannot reach.
const POLICIES: &str = include_str!("../../assets/policies.json");

/// Makes sure `paths.binary()` exists, fetching and unpacking the pinned build if not.
/// `progress(received, total)` is called as the download grows.
pub fn ensure(paths: &Paths, mut progress: impl FnMut(u64, u64)) -> Result<()> {
    let binary = paths.binary();
    if binary.is_file() {
        return policies(&paths.install);
    }
    if std::env::var_os("NOCTMALIA_THUNDERBIRD").is_some() {
        return Err(Error::local(format!("NOCTMALIA_THUNDERBIRD names {}, which does not exist", binary.display())));
    }
    if std::env::consts::ARCH != "x86_64" {
        return Err(Error::local(format!(
            "Mozilla ships Linux builds for x86_64 only, and this is {}. Install Thunderbird {VERSION} or newer \
             yourself and point NOCTMALIA_THUNDERBIRD at it.",
            std::env::consts::ARCH
        )));
    }
    std::fs::create_dir_all(&paths.cache)?;
    let tarball = paths.cache.join(format!("thunderbird-{VERSION}.tar.xz"));
    if !verified(&tarball) {
        download(&tarball, &mut progress)?;
        if !verified(&tarball) {
            let _ = std::fs::remove_file(&tarball);
            return Err(Error::local(format!("{} did not match the pinned SHA-512; deleted it", tarball.display())));
        }
    }
    let unpacked = paths.install.with_extension("unpacking");
    let _ = std::fs::remove_dir_all(&unpacked);
    std::fs::create_dir_all(&unpacked)?;
    let status = Command::new("tar")
        .args(["-xJf", &tarball.display().to_string(), "-C", &unpacked.display().to_string(), "--strip-components=1"])
        .stdin(Stdio::null())
        .status()?;
    if !status.success() {
        return Err(Error::local(format!("tar failed unpacking {}", tarball.display())));
    }
    std::fs::rename(&unpacked, &paths.install)?;
    eprintln!("noctmalia: unpacked Thunderbird {VERSION} into {}", paths.install.display());
    policies(&paths.install)
}

fn download(tarball: &Path, progress: &mut impl FnMut(u64, u64)) -> Result<()> {
    let partial = tarball.with_extension("xz.part");
    let _ = std::fs::remove_file(&partial);
    eprintln!("noctmalia: downloading Thunderbird {VERSION} ({} MB) to {}", BYTES / 1_000_000, tarball.display());
    let mut curl = Command::new("curl")
        .args(["-fsSL", "-o", &partial.display().to_string(), URL])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            Error::local(format!(
                "cannot run curl ({error}). Download {URL} yourself to {} and start again.",
                tarball.display()
            ))
        })?;
    loop {
        if let Some(status) = curl.try_wait()? {
            if !status.success() {
                let mut stderr = String::new();
                if let Some(mut pipe) = curl.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut pipe, &mut stderr);
                }
                return Err(Error::local(format!("curl failed: {}", stderr.trim())));
            }
            break;
        }
        let received = std::fs::metadata(&partial).map(|meta| meta.len()).unwrap_or(0);
        progress(received, BYTES);
        std::thread::sleep(Duration::from_millis(500));
    }
    progress(BYTES, BYTES);
    std::fs::rename(&partial, tarball)?;
    Ok(())
}

/// Whether the tarball is there and is the one that was pinned. `sha512sum` is coreutils.
fn verified(tarball: &Path) -> bool {
    if !tarball.is_file() {
        return false;
    }
    let output = Command::new("sha512sum").arg(tarball).stdin(Stdio::null()).output();
    output.is_ok_and(|output| {
        output.status.success() && String::from_utf8_lossy(&output.stdout).split_whitespace().next() == Some(SHA512)
    })
}

/// Writes `distribution/policies.json` into the install. Every start, since it is ours and cheap.
fn policies(install: &Path) -> Result<()> {
    let distribution = install.join("distribution");
    std::fs::create_dir_all(&distribution)?;
    let path = distribution.join("policies.json");
    if std::fs::read_to_string(&path).ok().as_deref() != Some(POLICIES) {
        std::fs::write(&path, POLICIES)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_binary_that_is_already_there_is_not_fetched_and_gets_its_policies() {
        let root = std::env::temp_dir().join(format!("noctmalia-fetch-test-{}", std::process::id()));
        let install = root.join("install");
        std::fs::create_dir_all(&install).unwrap();
        std::fs::write(install.join("thunderbird"), "#!/bin/sh\n").unwrap();
        let paths = Paths {
            install: install.clone(),
            profile: root.join("profile"),
            shim: root.join("nm-shim.py"),
            manifest: root.join("manifest.json"),
            cache: root.join("cache"),
            log: root.join("log"),
            socket: root.join("bridge.sock"),
        };
        let mut called = false;
        ensure(&paths, |_, _| called = true).expect("nothing to fetch");
        assert!(!called, "no download happened");
        assert_eq!(std::fs::read_to_string(install.join("distribution/policies.json")).unwrap(), POLICIES);
        let _ = std::fs::remove_dir_all(root);
    }
}
