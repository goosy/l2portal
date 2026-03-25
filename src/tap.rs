// src/tap.rs - TAP-Windows6 adapter lifecycle: create, configure, delete.
//
// tapctl.exe lookup order:
//   1. Same directory as l2portal.exe  (preferred; deployed by installer)
//   2. System PATH fallback            (allows dev/testing without full install)
// netsh is used for IP configuration and MTU.

use anyhow::{anyhow, Result};
use std::net::Ipv4Addr;
use std::process::Command;
use std::thread;
use std::time::Duration;

use crate::routing::prefix_to_mask;

const TAP_HWID: &str = "root\\tap0901";

/// Resolve the path to tapctl.exe.
fn tapctl_path() -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("tapctl.exe");
            if candidate.exists() {
                log::debug!("tapctl found alongside exe: {}", candidate.display());
                return candidate;
            }
        }
    }
    log::debug!("tapctl.exe not found alongside exe, falling back to PATH");
    std::path::PathBuf::from("tapctl.exe")
}

/// Create a new TAP adapter and return `(guid, actual_name)`.
pub fn tap_create(requested_name: Option<&str>) -> Result<(String, String)> {
    let tapctl = tapctl_path();
    log::info!("creating TAP adapter {:?}", requested_name);

    let mut cmd = Command::new(&tapctl);
    cmd.args(["create", "--hwid", TAP_HWID]);
    if let Some(name) = requested_name {
        cmd.args(["--name", name]);
    }

    let output = cmd
        .output()
        .map_err(|e| anyhow!("[ERROR] tap: failed to run tapctl.exe: {e}"))?;
    if !output.status.success() {
        return Err(anyhow!(
            "[ERROR] tap: tapctl create {:?} failed: {} stdout='{}' stderr='{}'",
            requested_name,
            output.status,
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let guid = parse_created_adapter(&stdout)?;
    let actual_name = tap_name_from_guid(&guid)?;

    let mtu_status = Command::new("netsh")
        .args([
            "interface",
            "ipv4",
            "set",
            "subinterface",
            &actual_name,
            "mtu=1400",
            "store=persistent",
        ])
        .status()
        .map_err(|e| anyhow!("[ERROR] tap: failed to run netsh (mtu): {e}"))?;
    if !mtu_status.success() {
        log::warn!(
            "netsh set mtu=1400 on '{}' failed: {}",
            actual_name,
            mtu_status
        );
    } else {
        log::info!("MTU=1400 set on '{}'", actual_name);
    }

    Ok((guid, actual_name))
}

/// Delete a TAP adapter instance by GUID.
pub fn tap_delete(guid: &str) -> Result<()> {
    let tapctl = tapctl_path();
    log::info!("deleting TAP adapter '{}'", guid);
    let status = Command::new(&tapctl)
        .args(["remove", guid])
        .status()
        .map_err(|e| anyhow!("[ERROR] tap: failed to run tapctl.exe: {e}"))?;
    if !status.success() {
        return Err(anyhow!(
            "[ERROR] tap: tapctl remove '{}' failed: {}",
            guid,
            status
        ));
    }
    Ok(())
}

/// Configure a static IPv4 address on the TAP adapter.
///
/// Must be called **after** `route_add_host` to avoid routing loops.
pub fn tap_set_ip(name: &str, ip: Ipv4Addr, prefix: u8) -> Result<()> {
    let mask = prefix_to_mask(prefix);
    log::info!("setting IP {}/{} on '{}'", ip, prefix, name);
    let status = Command::new("netsh")
        .args([
            "interface",
            "ip",
            "set",
            "address",
            name,
            "static",
            &ip.to_string(),
            &mask.to_string(),
        ])
        .status()
        .map_err(|e| anyhow!("[ERROR] tap: failed to run netsh (set ip): {e}"))?;
    if !status.success() {
        return Err(anyhow!(
            "[ERROR] tap: netsh set address on '{}' failed: {}",
            name,
            status
        ));
    }
    Ok(())
}

/// Switch the TAP adapter back to DHCP, clearing the static IP.
pub fn tap_clear_ip(name: &str) -> Result<()> {
    log::info!("clearing IP on '{}' (setting DHCP)", name);
    let _ = Command::new("netsh")
        .args(["interface", "ip", "set", "address", name, "dhcp"])
        .status();
    Ok(())
}

fn parse_created_adapter(stdout: &str) -> Result<String> {
    for line in stdout.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let (Some(open), Some(close)) = (line.find('{'), line.find('}')) {
            if close > open {
                return Ok(line[open..=close].to_string());
            }
        }
    }
    Err(anyhow!("tapctl create output did not include adapter GUID"))
}

pub fn tap_name_from_guid(guid: &str) -> Result<String> {
    for _ in 0..20 {
        if let Some(name) = tap_name_from_guid_once(guid)? {
            return Ok(name);
        }
        thread::sleep(Duration::from_millis(200));
    }

    Err(anyhow!(
        "unable to resolve adapter name for GUID '{}' after TAP creation",
        guid
    ))
}

fn tap_name_from_guid_once(guid: &str) -> Result<Option<String>> {
    let script = format!(
        "$adapter = Get-NetAdapter -IncludeHidden | Where-Object {{ $_.InterfaceGuid -eq '{}' }} | Select-Object -First 1 -ExpandProperty Name; if ($adapter) {{ Write-Output $adapter }}",
        guid
    );
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .map_err(|e| anyhow!("[ERROR] tap: failed to run powershell (Get-NetAdapter): {e}"))?;
    if !output.status.success() {
        return Err(anyhow!(
            "[ERROR] tap: Get-NetAdapter lookup for '{}' failed: {} stdout='{}' stderr='{}'",
            guid,
            output.status,
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
    }

    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() {
        Ok(None)
    } else {
        Ok(Some(name))
    }
}
