//! Update checks against the project's GitHub releases, and in-place
//! installs: the new app replaces the running one, then relaunches.

use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

const LATEST: &str = "https://api.github.com/repos/xanaxfather/marquee/releases/latest";
pub const RELEASES_PAGE: &str = "https://github.com/xanaxfather/marquee/releases";

#[cfg(target_os = "macos")]
const ASSET: &str = "marquee-macos.zip";
#[cfg(windows)]
const ASSET: &str = "marquee-windows-x64.zip";
#[cfg(not(any(target_os = "macos", windows)))]
const ASSET: &str = "";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub page: String,
    /// Download for this platform, if the release has one.
    pub asset: Option<String>,
}

fn agent() -> ureq::Agent {
    let mut b = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(60))
        .user_agent(concat!("marquee/", env!("CARGO_PKG_VERSION")));
    if let Ok(tls) = native_tls::TlsConnector::new() {
        b = b.tls_connector(std::sync::Arc::new(tls));
    }
    b.build()
}

/// "v1.2.3" → (1, 2, 3); missing parts are 0, anything after "-" is ignored.
fn parse_version(v: &str) -> (u64, u64, u64) {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let core = v.split(['-', '+']).next().unwrap_or("");
    let mut it = core.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0), it.next().unwrap_or(0))
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    parse_version(candidate) > parse_version(current)
}

/// The latest release if it is newer than this build.
pub fn check() -> Result<Option<Release>> {
    let v: serde_json::Value = match agent().get(LATEST).set("Accept", "application/vnd.github+json").call() {
        Ok(r) => r.into_json()?,
        Err(ureq::Error::Status(404, _)) => return Ok(None), // no releases yet
        Err(e) => return Err(e).context("checking GitHub releases"),
    };
    let tag = v["tag_name"].as_str().unwrap_or("");
    let current = env!("CARGO_PKG_VERSION");
    tracing::info!(latest = tag, current, "update check");
    if tag.is_empty() || !is_newer(tag, current) {
        return Ok(None);
    }
    let asset = v["assets"].as_array().and_then(|a| {
        a.iter()
            .find(|x| x["name"].as_str() == Some(ASSET))
            .and_then(|x| x["browser_download_url"].as_str())
            .filter(|u| u.starts_with("https://github.com/"))
            .map(String::from)
    });
    Ok(Some(Release {
        version: tag.trim_start_matches('v').to_string(),
        notes: v["body"].as_str().unwrap_or("").chars().take(8000).collect(),
        page: v["html_url"].as_str().filter(|u| u.starts_with("https://github.com/")).unwrap_or(RELEASES_PAGE).into(),
        asset,
    }))
}

fn download(url: &str, to: &Path) -> Result<()> {
    let resp = agent().get(url).call().context("downloading the update")?;
    let mut bytes = Vec::new();
    resp.into_reader().take(200 << 20).read_to_end(&mut bytes)?;
    if bytes.len() < 1024 || !bytes.starts_with(b"PK") {
        bail!("the download is not a zip file");
    }
    std::fs::write(to, bytes)?;
    Ok(())
}

/// The .app bundle we are running from (macOS), as it sits on disk.
#[cfg(target_os = "macos")]
fn current_bundle() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let bundle = exe
        .ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .map(Path::to_path_buf)
        .context("not running from Marquee.app; download the update from the releases page")?;
    if !is_translocated(&bundle) {
        return Ok(bundle);
    }
    let original = original_path(&bundle).context(
        "macOS is running Marquee from a temporary read-only copy. Move Marquee.app to your Applications folder, open it from there, then try again",
    )?;
    tracing::info!(running = %bundle.display(), original = %original.display(), "app is translocated; updating the original");
    Ok(original)
}

/// Gatekeeper runs a quarantined app that was opened where it was downloaded
/// from a randomised read-only mount ("App Translocation").
#[cfg(target_os = "macos")]
fn is_translocated(p: &Path) -> bool {
    p.to_string_lossy().contains("/AppTranslocation/")
}

/// Where a translocated app really lives, via the Security framework call
/// Apple provides for exactly this (looked up at run time; it is not in the
/// public headers).
#[cfg(target_os = "macos")]
fn original_path(translocated: &Path) -> Option<PathBuf> {
    use core_foundation::base::TCFType;
    use core_foundation::url::CFURL;
    use core_foundation_sys::url::CFURLRef;
    type Func = unsafe extern "C" fn(CFURLRef, *mut *mut std::ffi::c_void) -> CFURLRef;

    let url = CFURL::from_path(translocated, true)?;
    // SAFETY: dlopen/dlsym with NUL-terminated names; the symbol has the
    // signature `CFURLRef (CFURLRef, CFErrorRef *)` and returns a +1 reference.
    unsafe {
        let lib = libc::dlopen(c"/System/Library/Frameworks/Security.framework/Security".as_ptr(), libc::RTLD_LAZY);
        if lib.is_null() {
            return None;
        }
        let sym = libc::dlsym(lib, c"SecTranslocateCreateOriginalPathForURL".as_ptr());
        if sym.is_null() {
            return None;
        }
        let f: Func = std::mem::transmute(sym);
        let out = f(url.as_concrete_TypeRef(), std::ptr::null_mut());
        if out.is_null() {
            return None;
        }
        let path = CFURL::wrap_under_create_rule(out).to_path()?;
        (!is_translocated(&path) && path.extension().is_some_and(|e| e == "app")).then_some(path)
    }
}

/// Download, verify and swap in the new version, then start it once this
/// process exits. The caller quits right after.
pub fn install(release: &Release) -> Result<()> {
    let url = release.asset.as_deref().context("this release has no download for your system")?;
    tracing::info!(version = release.version, url, "installing update");
    install_platform(url)
}

#[cfg(target_os = "macos")]
fn install_platform(url: &str) -> Result<()> {
    use std::process::Command;
    let bundle = current_bundle()?;
    let parent = bundle.parent().context("app folder")?.to_path_buf();
    // Work next to the app so the final moves are renames on one volume.
    let work = parent.join(".marquee-update");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).with_context(|| {
        format!(
            "Marquee can't write to {}. Move Marquee.app to your Applications folder and try again",
            parent.display()
        )
    })?;
    let zip = work.join("update.zip");
    download(url, &zip)?;
    let status = Command::new("/usr/bin/ditto").args(["-x", "-k"]).arg(&zip).arg(&work).status()?;
    if !status.success() {
        bail!("could not unpack the update");
    }
    let new_app = work.join("Marquee.app");
    if !new_app.join("Contents/MacOS/marquee").is_file() {
        bail!("the update does not contain Marquee.app");
    }
    // A quarantined copy would be translocated again on its next launch.
    let _ = Command::new("/usr/bin/xattr").args(["-dr", "com.apple.quarantine"]).arg(&new_app).status();
    let old = work.join("Marquee-old.app");
    std::fs::rename(&bundle, &old).context("moving the current app aside")?;
    if let Err(e) = std::fs::rename(&new_app, &bundle) {
        let _ = std::fs::rename(&old, &bundle);
        return Err(e).context("moving the new app into place");
    }
    // Relaunch once this process is gone, then clean up.
    let script = format!(
        "while kill -0 {pid} 2>/dev/null; do sleep 0.2; done; /usr/bin/open {app}; rm -rf {work}",
        pid = std::process::id(),
        app = shell_quote(&bundle),
        work = shell_quote(&work),
    );
    Command::new("/bin/sh").args(["-c", &script]).spawn().context("scheduling the relaunch")?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn shell_quote(p: &Path) -> String {
    format!("'{}'", p.to_string_lossy().replace('\'', "'\\''"))
}

#[cfg(windows)]
fn install_platform(url: &str) -> Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let exe = std::env::current_exe()?;
    let dir = exe.parent().context("app folder")?.to_path_buf();
    let work = dir.join(".marquee-update");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).with_context(|| format!("no write access to {}", dir.display()))?;
    let zip = work.join("update.zip");
    download(url, &zip)?;
    let status = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", "Expand-Archive", "-Force", "-LiteralPath"])
        .arg(&zip)
        .arg("-DestinationPath")
        .arg(&work)
        .creation_flags(CREATE_NO_WINDOW)
        .status()?;
    let new_exe = work.join("marquee.exe");
    if !status.success() || !new_exe.is_file() {
        bail!("could not unpack the update");
    }
    // A running .exe can be renamed but not overwritten.
    let old = old_exe(&exe);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).context("moving the current version aside")?;
    if let Err(e) = std::fs::rename(&new_exe, &exe) {
        let _ = std::fs::rename(&old, &exe);
        return Err(e).context("moving the new version into place");
    }
    let script = format!(
        "Wait-Process -Id {} -ErrorAction SilentlyContinue; Start-Process -FilePath '{}'; Remove-Item -Recurse -Force -LiteralPath '{}'",
        std::process::id(),
        exe.display().to_string().replace('\'', "''"),
        work.display().to_string().replace('\'', "''"),
    );
    Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .context("scheduling the relaunch")?;
    Ok(())
}

#[cfg(windows)]
fn old_exe(exe: &Path) -> PathBuf {
    exe.with_extension("old.exe")
}

#[cfg(not(any(target_os = "macos", windows)))]
fn install_platform(_url: &str) -> Result<()> {
    bail!("automatic updates are not supported on this system")
}

/// Remove what a previous update left behind (the old Windows .exe).
pub fn cleanup() {
    #[cfg(windows)]
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(old_exe(&exe));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(is_newer("v0.3.0", "0.2.0"));
        assert!(is_newer("0.2.10", "0.2.9"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("v0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        assert!(!is_newer("garbage", "0.2.0"));
    }
}
