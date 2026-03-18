// src/state.rs — Startup state file for TAP/route residue cleanup.
//
// File location: %APPDATA%\L2Portal\state
// Format (key=value, one per line):
//   tap_name=tap-ot
//   tap_route=203.0.113.1   (optional; present only when IP/prefix was given)
//
// On startup, any existing state file signals a crash-residue; the program
// cleans up the old TAP and route before continuing with normal startup.

use anyhow::Result;
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::PathBuf;

/// Returns the canonical state file path.
pub fn state_path() -> PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(appdata).join("L2Portal").join("state")
}

/// Write a new state file recording the current TAP name and optional route.
///
/// `tap_route` is the destination IP of the injected host route
/// (i.e. the `--remote` IP, not the gateway).
pub fn state_write(tap_name: &str, tap_route: Option<Ipv4Addr>) -> Result<()> {
    let path = state_path();
    let dir = path.parent().ok_or_else(|| {
        anyhow::anyhow!("state: cannot determine state directory from path '{}'", path.display())
    })?;
    std::fs::create_dir_all(dir)
        .map_err(|e| anyhow::anyhow!("state: cannot create state dir '{}': {e}", dir.display()))?;

    let mut content = format!("tap_name={}\n", tap_name);
    if let Some(ip) = tap_route {
        content.push_str(&format!("tap_route={}\n", ip));
    }
    std::fs::write(&path, &content)
        .map_err(|e| anyhow::anyhow!("[ERROR] state: cannot write state file: {e}"))?;
    log::debug!("state file written to {}", path.display());
    Ok(())
}

/// Remove the state file on clean exit.
pub fn state_remove() {
    let path = state_path();
    if path.exists() {
        if let Err(e) = std::fs::remove_file(&path) {
            log::warn!("failed to remove state file: {e}");
        } else {
            log::debug!("state file removed");
        }
    }
}

/// On startup, check for a leftover state file and clean up any residue.
///
/// Cleanup order: delete host route → delete TAP adapter → delete state file.
/// Each step is attempted independently; failures are logged as WARN and do
/// not abort the remaining cleanup steps.
pub fn cleanup_residue() {
    let path = state_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return, // No state file; nothing to clean up.
    };

    let fields: HashMap<&str, &str> = text
        .lines()
        .filter_map(|l| l.split_once('='))
        .collect();

    let tap_name = match fields.get("tap_name") {
        Some(&n) => n,
        None => {
            log::warn!("state file exists but has no tap_name field");
            let _ = std::fs::remove_file(&path);
            return;
        }
    };

    log::info!(
        "residue detected — cleaning up TAP '{}'",
        tap_name
    );

    // Step 1: delete the host route if recorded.
    if let Some(&ip_str) = fields.get("tap_route") {
        match ip_str.parse::<Ipv4Addr>() {
            Ok(ip) => {
                if let Err(e) = crate::routing::route_delete_host(ip) {
                    log::warn!("residue route delete failed: {e}");
                }
            }
            Err(e) => {
                log::warn!("cannot parse tap_route '{}': {e}", ip_str);
            }
        }
    }

    // Step 2: delete the TAP adapter.
    if let Err(e) = crate::tap::tap_delete(tap_name) {
        log::warn!("residue TAP delete failed: {e}");
    }

    // Step 3: remove the state file regardless of the above outcomes.
    let _ = std::fs::remove_file(&path);
    log::info!("residue cleanup complete");
}
