// src/tap.rs — TAP-Windows6 adapter lifecycle: create, configure, delete.
//
// tapctl.exe lookup order:
//   1. Same directory as l2portal.exe  (preferred; deployed by installer)
//   2. System PATH fallback            (allows dev/testing without full install)
// netsh is used for IP configuration and MTU.

use anyhow::{anyhow, Result};
use std::net::Ipv4Addr;
use std::process::Command;

use crate::routing::prefix_to_mask;

/// Resolve the path to tapctl.exe.
///
/// Search order:
///   1. Directory containing l2portal.exe — the normal installed layout.
///   2. System PATH fallback — for development or non-standard installs.
///
/// Returns a bare filename "tapctl.exe" for the PATH case; Command::new
/// will invoke the OS search at spawn time, and the OS error message will
/// be clear if it is not found there either.
fn tapctl_path() -> std::path::PathBuf {
    // 1. Prefer the exe-adjacent copy.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("tapctl.exe");
            if candidate.exists() {
                log::debug!("tapctl found alongside exe: {}", candidate.display());
                return candidate;
            }
        }
    }
    // 2. Fall back to PATH resolution at spawn time.
    log::debug!("tapctl.exe not found alongside exe, falling back to PATH");
    std::path::PathBuf::from("tapctl.exe")
}

/// Create a new TAP adapter instance with the given name and set MTU=1400.
///
/// Any pre-existing residual instance is expected to have been cleaned up
/// by `state::cleanup_residue()` before this call.
pub fn tap_create(name: &str) -> Result<()> {
    let tapctl = tapctl_path();
    log::info!("creating TAP adapter '{}'", name);

    let status = Command::new(&tapctl)
        .args(["create", "--name", name])
        .status()
        .map_err(|e| anyhow!("[ERROR] tap: failed to run tapctl.exe: {e}"))?;
    if !status.success() {
        return Err(anyhow!(
            "[ERROR] tap: tapctl create '{}' failed: {}",
            name,
            status
        ));
    }

    // Set MTU to 1400 to prevent oversized frames from entering the tunnel.
    let mtu_status = Command::new("netsh")
        .args([
            "interface",
            "ipv4",
            "set",
            "subinterface",
            name,
            "mtu=1400",
            "store=persistent",
        ])
        .status()
        .map_err(|e| anyhow!("[ERROR] tap: failed to run netsh (mtu): {e}"))?;
    if !mtu_status.success() {
        log::warn!("netsh set mtu=1400 on '{}' failed: {}", name, mtu_status);
        // Non-fatal: continue even if MTU set fails.
    } else {
        log::info!("MTU=1400 set on '{}'", name);
    }

    Ok(())
}

/// Delete a TAP adapter instance by name.
pub fn tap_delete(name: &str) -> Result<()> {
    let tapctl = tapctl_path();
    log::info!("deleting TAP adapter '{}'", name);
    let status = Command::new(&tapctl)
        .args(["delete", "--name", name])
        .status()
        .map_err(|e| anyhow!("[ERROR] tap: failed to run tapctl.exe: {e}"))?;
    if !status.success() {
        return Err(anyhow!(
            "[ERROR] tap: tapctl delete '{}' failed: {}",
            name,
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
///
/// Called during cleanup to avoid triggering route recalculation before
/// the host route is removed.
pub fn tap_clear_ip(name: &str) -> Result<()> {
    log::info!("clearing IP on '{}' (setting DHCP)", name);
    let _ = Command::new("netsh")
        .args(["interface", "ip", "set", "address", name, "dhcp"])
        .status();
    Ok(())
}
