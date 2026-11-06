//! Read/modify IINA's "Additional mpv options" so it exposes the IPC socket.
//!
//! IINA stores them under `userOptions` (array of `[name, value]` pairs) and
//! only applies them when `enableAdvancedSettings` is on.

use core_foundation::array::CFArray;
use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::string::CFString;
use core_foundation_sys::preferences::{
    CFPreferencesAppSynchronize, CFPreferencesCopyAppValue, CFPreferencesSetAppValue,
};
use std::path::Path;

const IINA_DOMAIN: &str = "com.colliderli.iina";
const KEY_OPTIONS: &str = "userOptions";
const KEY_ADVANCED: &str = "enableAdvancedSettings";
const IPC_OPTION: &str = "input-ipc-server";

fn domain() -> CFString {
    CFString::new(IINA_DOMAIN)
}

fn copy(key: &str) -> Option<CFType> {
    let k = CFString::new(key);
    let d = domain();
    unsafe {
        CFPreferencesAppSynchronize(d.as_concrete_TypeRef());
        let v = CFPreferencesCopyAppValue(k.as_concrete_TypeRef(), d.as_concrete_TypeRef());
        (!v.is_null()).then(|| CFType::wrap_under_create_rule(v))
    }
}

fn cf_to_string(v: &CFType) -> Option<String> {
    v.downcast::<CFString>().map(|s| s.to_string())
}

fn array_items(v: &CFType) -> Option<Vec<CFType>> {
    let arr = v.downcast::<CFArray>()?;
    // SAFETY: elements of a CFArray are valid CF objects for the array's lifetime;
    // wrap_under_get_rule retains them.
    Some(arr.iter().map(|p| unsafe { CFType::wrap_under_get_rule(*p) }).collect())
}

pub fn user_options() -> Vec<(String, String)> {
    let Some(v) = copy(KEY_OPTIONS) else { return vec![] };
    let Some(items) = array_items(&v) else { return vec![] };
    items
        .iter()
        .filter_map(|item| {
            let pair = array_items(item)?;
            let name = cf_to_string(pair.first()?)?;
            let value = pair.get(1).and_then(cf_to_string).unwrap_or_default();
            Some((name, value))
        })
        .collect()
}

pub fn advanced_enabled() -> bool {
    copy(KEY_ADVANCED).and_then(|v| v.downcast::<CFBoolean>()).map(bool::from).unwrap_or(false)
}

pub fn is_installed() -> bool {
    Path::new("/Applications/IINA.app").exists() || crate::paths::home().join("Applications/IINA.app").exists()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupState {
    Ready,
    /// Option present but advanced settings are switched off.
    AdvancedDisabled,
    /// Points somewhere else.
    WrongSocket(String),
    Missing,
}

pub fn check(socket: &Path) -> SetupState {
    let want = socket.to_string_lossy();
    let current = user_options().into_iter().find(|(k, _)| k == IPC_OPTION).map(|(_, v)| v);
    match current {
        None => SetupState::Missing,
        Some(v) if v != want => SetupState::WrongSocket(v),
        Some(_) if !advanced_enabled() => SetupState::AdvancedDisabled,
        Some(_) => SetupState::Ready,
    }
}

/// Add/replace `input-ipc-server` and enable advanced settings, keeping the
/// user's other mpv options intact. IINA picks it up for new player windows;
/// a restart of IINA guarantees it.
pub fn configure(socket: &Path) -> anyhow::Result<()> {
    let mut opts: Vec<(String, String)> = user_options().into_iter().filter(|(k, _)| k != IPC_OPTION).collect();
    opts.push((IPC_OPTION.to_string(), socket.to_string_lossy().into_owned()));

    let pairs: Vec<CFArray<CFString>> =
        opts.iter().map(|(k, v)| CFArray::from_CFTypes(&[CFString::new(k), CFString::new(v)])).collect();
    let array = CFArray::from_CFTypes(&pairs);
    let d = domain();
    unsafe {
        CFPreferencesSetAppValue(
            CFString::new(KEY_OPTIONS).as_concrete_TypeRef(),
            array.as_CFTypeRef(),
            d.as_concrete_TypeRef(),
        );
        CFPreferencesSetAppValue(
            CFString::new(KEY_ADVANCED).as_concrete_TypeRef(),
            CFBoolean::true_value().as_CFTypeRef(),
            d.as_concrete_TypeRef(),
        );
        if CFPreferencesAppSynchronize(d.as_concrete_TypeRef()) == 0 {
            anyhow::bail!("CFPreferencesAppSynchronize failed for {IINA_DOMAIN}");
        }
    }
    tracing::info!(socket = %socket.display(), other_options = opts.len() - 1, "configured IINA mpv IPC option");
    Ok(())
}
