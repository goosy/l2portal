// src/client.rs — Client mode: bridge a TAP virtual adapter to a UDP tunnel.
//
// The TAP adapter is created on startup and destroyed on exit (including Ctrl+C).
// Runtime peer switching is supported by writing to stdin: "switch <IP:PORT>"
//
// Cleanup order on exit:
//   tap_clear_ip → route_delete_host → tap_delete → state_remove

use anyhow::{Context, Result, anyhow};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UdpSocket;
use tokio::sync::RwLock;

use crate::routing;
use crate::state;
use crate::tap;

/// Parameters for client mode.
pub struct ClientParams {
    pub tap_name: String,
    pub tap_ip_prefix: Option<(Ipv4Addr, u8)>,
    pub local_addr: SocketAddr,
    pub remote_addr: SocketAddr,
}

/// Run the client mode event loop.
pub async fn run(params: ClientParams) -> Result<()> {
    let ClientParams {
        tap_name,
        tap_ip_prefix,
        local_addr,
        remote_addr,
    } = params;

    log::info!(
        "starting — tap={} local={} remote={}",
        tap_name,
        local_addr,
        remote_addr
    );

    // ── Startup residue cleanup ─────────────────────────────────────────────
    state::cleanup_residue();

    // ── Create TAP adapter ──────────────────────────────────────────────────
    let tap_display_name = tap_name.clone();
    let requested_tap_name = if tap_name.eq_ignore_ascii_case("auto") {
        None
    } else {
        Some(tap_name.as_str())
    };
    let (tap_guid, tap_name) = tap::tap_create(requested_tap_name)
        .with_context(|| format!("[ERROR] client: tap_create '{}' failed", tap_display_name))?;
    if let Some(name) = requested_tap_name {
        if tap_name != name {
            log::warn!(
                "tapctl created adapter name '{}' but requested name was '{}'",
                tap_name,
                name
            );
        }
    }

    // Write TAP GUID to state file immediately (route not yet recorded).
    state::state_write(&tap_guid, None)?;

    // ── Inject host route and optionally configure TAP IP ──────────────────
    let remote_ip = remote_ipv4(remote_addr)?;

    sync_tap_route(&tap_guid, None, remote_ip)?;

    if let Some((tap_ip, prefix)) = tap_ip_prefix {
        // configure TAP IP (after underlay route is pinned).
        tap::tap_set_ip(&tap_name, tap_ip, prefix)
            .context("[ERROR] client: tap_set_ip failed")?;
    }

    // ── Open TAP device for read/write ─────────────────────────────────────
    // tap-windows crate: open by adapter name (friendly name).
    // Device implements std::io::Read + Write on &mut self, so we need
    // exclusive access per operation.  We use two separate Device handles
    // (one for reading, one for writing) obtained from the same underlying
    // Windows file object opened twice, which is the idiomatic approach with
    // the tap-windows crate — the OS supports concurrent overlapped I/O on
    // the same device via two independent handles.
    // Open a short-lived control handle to bring the TAP up, then drop it.
    let tap_ctrl = tap_windows::Device::open(&tap_name)
        .with_context(|| format!("[ERROR] client: failed to open TAP device '{}' (ctrl)", tap_name))?;
    tap_ctrl
        .set_status(true)
        .context("[ERROR] client: failed to set TAP status up")?;
    drop(tap_ctrl);

    // ── Bind UDP socket ─────────────────────────────────────────────────────
    let udp = Arc::new(
        UdpSocket::bind(local_addr)
            .await
            .with_context(|| format!("[ERROR] client: UDP bind {} failed", local_addr))?,
    );
    log::info!("UDP socket bound on {}", local_addr);

    // Shared remote address (updated atomically by the stdin task).
    let remote_shared: Arc<RwLock<SocketAddr>> = Arc::new(RwLock::new(remote_addr));

    // ── Thread 1 & Task 1: UDP RX → TAP write ─────────────────────────────
    // UDP receive runs as a tokio task; TAP writes run in a blocking thread
    // (tap-windows Device::write is blocking/synchronous).
    let (udp_to_tap_tx, mut udp_to_tap_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);

    let udp_rx = udp.clone();
    let task_udp_rx = tokio::spawn(async move {
        let mut buf = vec![0u8; 1600];
        loop {
            match udp_rx.recv_from(&mut buf).await {
                Ok((n, _src)) => {
                    if udp_to_tap_tx.send(buf[..n].to_vec()).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    log::error!("UDP recv_from failed: {e}");
                    break;
                }
            }
        }
        log::warn!("UDP-RX task exiting");
    });

    // Blocking thread for TAP writes.
    // Blocking thread for TAP writes — open a dedicated handle inside the thread
    {
        let tap_name_for_write = tap_name.clone();
        std::thread::Builder::new()
            .name("tap-write".into())
            .spawn(move || {
                use std::io::Write;
                let mut dev = match tap_windows::Device::open(&tap_name_for_write) {
                    Ok(d) => d,
                    Err(e) => {
                        eprintln!("[ERROR] client: failed to open TAP device for write: {e}");
                        return;
                    }
                };
                while let Some(frame) = udp_to_tap_rx.blocking_recv() {
                    if let Err(e) = dev.write_all(&frame) {
                        eprintln!("[ERROR] client: TAP write failed: {e}");
                        break;
                    }
                }
            })
            .context("[ERROR] client: failed to spawn tap-write thread")?;
    }

    // ── Thread 2 & Task 2: TAP read → UDP TX ──────────────────────────────
    let (tap_to_udp_tx, mut tap_to_udp_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);

    // Blocking thread for TAP reads.
    // Blocking thread for TAP reads — open a dedicated handle inside the thread
    {
        let tap_name_for_read = tap_name.clone();
        std::thread::Builder::new()
            .name("tap-read".into())
            .spawn(move || {
                use std::io::Read;
                let mut dev = match tap_windows::Device::open(&tap_name_for_read) {
                    Ok(d) => d,
                    Err(e) => {
                        eprintln!("[ERROR] client: failed to open TAP device for read: {e}");
                        return;
                    }
                };
                let mut buf = vec![0u8; 1600];
                loop {
                    match dev.read(&mut buf) {
                        Ok(0) => continue,
                        Ok(n) => {
                            if tap_to_udp_tx.blocking_send(buf[..n].to_vec()).is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            eprintln!("[ERROR] client: TAP read failed: {e}");
                            break;
                        }
                    }
                }
            })
            .context("[ERROR] client: failed to spawn tap-read thread")?;
    }

    let udp_tx = udp.clone();
    let remote_for_tx = remote_shared.clone();
    let task_udp_tx = tokio::spawn(async move {
        while let Some(frame) = tap_to_udp_rx.recv().await {
            let dest = *remote_for_tx.read().await;
            if let Err(e) = udp_tx.send_to(&frame, dest).await {
                log::error!("UDP send_to failed: {e}");
                break;
            }
        }
        log::warn!("UDP-TX task exiting");
    });

    // ── Task 3: stdin — runtime peer switching ─────────────────────────────
    let remote_for_stdin = remote_shared.clone();
    let tap_guid_for_stdin = tap_guid.clone();
    let task_stdin = tokio::spawn(async move {
        let stdin = tokio::io::stdin();
        let mut lines = BufReader::new(stdin).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let line = line.trim().to_string();
            if let Some(addr_str) = line.strip_prefix("switch ") {
                match addr_str.trim().parse::<SocketAddr>() {
                    Ok(new_addr) => {
                        let old_addr = *remote_for_stdin.read().await;
                        let old_ip = match remote_ipv4(old_addr) {
                            Ok(ip) => ip,
                            Err(e) => {
                                log::warn!("{e}");
                                eprintln!("{e}");
                                continue;
                            }
                        };
                        let new_ip = match remote_ipv4(new_addr) {
                            Ok(ip) => ip,
                            Err(e) => {
                                log::warn!("{e}");
                                eprintln!("{e}");
                                continue;
                            }
                        };
                        if let Err(e) = sync_tap_route(&tap_guid_for_stdin, Some(old_ip), new_ip) {
                            log::warn!("switch route update failed: {e}");
                            eprintln!("[WARN] client: switch route update failed: {e}");
                            continue;
                        }
                        *remote_for_stdin.write().await = new_addr;
                        log::info!("switched remote to {}", new_addr);
                        eprintln!("[INFO] client: switched remote to {new_addr}");
                    }
                    Err(e) => {
                        log::warn!("invalid address '{}': {e}", addr_str);
                        eprintln!("[WARN] client: invalid address '{addr_str}': {e}");
                    }
                }
            } else if !line.is_empty() {
                eprintln!("[WARN] client: unknown command '{}' (hint: switch <IP:PORT>)", line);
            }
        }
        log::info!("stdin closed");
    });

    // ── Register Ctrl+C cleanup ─────────────────────────────────────────────
    let tap_name_ctrlc = tap_name.clone();
    let tap_guid_ctrlc = tap_guid.clone();
    let remote_for_ctrlc = remote_shared.clone();
    let ctrlc_handler = tokio::spawn(async move {
        if let Ok(()) = tokio::signal::ctrl_c().await {
            eprintln!("[INFO] client: Ctrl+C received — cleaning up");
            let remote_ip = current_remote_ip(&remote_for_ctrlc);
            cleanup(tap_name_ctrlc.as_str(), tap_guid_ctrlc.as_str(), remote_ip);
            std::process::exit(0);
        }
    });

    // ── Wait for any task to exit ───────────────────────────────────────────
    tokio::select! {
        _ = task_udp_rx  => { log::warn!("UDP-RX task ended unexpectedly"); }
        _ = task_udp_tx  => { log::warn!("UDP-TX task ended unexpectedly"); }
        _ = task_stdin   => { log::info!("stdin task ended"); }
        _ = ctrlc_handler => {}
    }

    // ── Normal exit cleanup ─────────────────────────────────────────────────
    let remote_ip = current_remote_ip(&remote_shared);
    cleanup(&tap_name, &tap_guid, remote_ip);
    Ok(())
}

fn remote_ipv4(addr: SocketAddr) -> Result<Ipv4Addr> {
    match addr.ip() {
        std::net::IpAddr::V4(ip) => Ok(ip),
        _ => Err(anyhow!("[ERROR] client: remote address must be IPv4")),
    }
}

fn current_remote_ip(remote: &Arc<RwLock<SocketAddr>>) -> Ipv4Addr {
    match remote.try_read() {
        Ok(addr) => remote_ipv4(*addr).unwrap_or(Ipv4Addr::UNSPECIFIED),
        Err(_) => Ipv4Addr::UNSPECIFIED,
    }
}

/// Keep the underlay host route aligned with the current remote peer and persist it in state.
fn sync_tap_route(tap_guid: &str, old_remote: Option<Ipv4Addr>, new_remote: Ipv4Addr) -> Result<()> {
    if old_remote == Some(new_remote) {
        routing::route_delete_host(new_remote)
            .context("[ERROR] client: route_delete_host failed")?;
    }
    let best = routing::get_best_route(new_remote)
        .context("[ERROR] client: cannot determine outbound route")?;
    routing::route_add_host(new_remote, best.gateway, best.if_index)
        .context("[ERROR] client: route_add_host failed")?;
    state::state_write(tap_guid, Some(new_remote))?;
    if let Some(old_remote) = old_remote.filter(|old_remote| *old_remote != new_remote) {
        routing::route_delete_host(old_remote)
            .context("[ERROR] client: route_delete_host failed")?;
    }
    Ok(())
}

/// Perform cleanup in the correct order:
///   1. Clear TAP IP  (avoids route-recalc before host route removal)
///   2. Delete host route
///   3. Delete TAP adapter
///   4. Remove state file
fn cleanup(tap_name: &str, tap_guid: &str, remote_ip: Ipv4Addr) {
    if let Err(e) = tap::tap_clear_ip(tap_name) {
        log::warn!("tap_clear_ip failed: {e}");
    }
    if let Err(e) = routing::route_delete_host(remote_ip) {
        log::warn!("route_delete_host failed: {e}");
    }
    if let Err(e) = tap::tap_delete(tap_guid) {
        log::warn!("tap_delete failed: {e}");
    }
    state::state_remove();
    log::info!("cleanup complete");
}
