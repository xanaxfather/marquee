//! Turn on MPC-HC's web interface (localhost only) in the registry.
//! MPC-BE: enable Options → Web Interface by hand (same default port).

use crate::config::Config;
use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;

const KEY: &str = r"Software\MPC-HC\MPC-HC";

pub fn is_installed() -> bool {
    RegKey::predef(HKEY_CURRENT_USER).open_subkey(KEY).is_ok()
}

pub fn is_ready(cfg: &Config) -> bool {
    let Ok(settings) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(format!(r"{KEY}\Settings")) else {
        return false;
    };
    let on: u32 = settings.get_value("EnableWebServer").unwrap_or(0);
    let port: u32 = settings.get_value("WebServerPort").unwrap_or(13579);
    on == 1 && port == cfg.mpc_port as u32
}

pub fn configure(cfg: &mut Config) -> anyhow::Result<()> {
    let (settings, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(format!(r"{KEY}\Settings"))?;
    settings.set_value("EnableWebServer", &1u32)?;
    settings.set_value("WebServerPort", &(cfg.mpc_port as u32))?;
    settings.set_value("WebServerLocalhostOnly", &1u32)?;
    Ok(())
}
