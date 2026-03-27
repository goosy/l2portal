// src/server.rs — Server mode: bridge a physical NIC to a UDP tunnel.
//
// Captures all Ethernet frames from the specified interface in promiscuous
// mode and forwards them as raw UDP payloads to the remote peer.
// Frames received from the remote peer are injected back into the NIC.
//
// UDP encapsulation format: bare Ethernet frame as UDP payload (no extra header).
// This is wire-compatible with l2tunnel and similar tools.
//
// Because pcap::Capture uses a blocking C API, capture and injection each run
// in their own OS thread, communicating with the async runtime via mpsc channels.
//
// An optional BPF filter can be passed in via `capture_filter` to suppress
// unwanted frames before they reach the forwarding path.

use anyhow::{anyhow, Context, Result};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::net::UdpSocket;

/// Windows WSAECONNRESET: returned by Winsock when an outgoing UDP packet
/// triggers an ICMP Port Unreachable reply from the remote host.  This means
/// the remote IP is reachable but the target port is not yet open (peer not
/// started / not ready).  It is non-fatal for a UDP tunnel; we continue and
/// use it as a coarse "peer IP-reachable but port closed" signal.
#[cfg(target_os = "windows")]
const WSAECONNRESET: i32 = 10054;

/// Ethernet frame buffer size: standard MTU 1500 + header 14 + FCS 4 = 1518.
const FRAME_BUF: usize = 1518;

/// UDP receive buffer: slightly larger than max frame to detect oversized packets.
const UDP_BUF: usize = 1600;

/// Run the server mode event loop.
///
/// `pcap_device`    — npcap device path resolved by `iface::resolve_iface`.
/// `local_addr`     — UDP socket bind address (already resolved from routing if 0.0.0.0).
/// `remote_addr`    — fixed remote peer address.
/// `capture_filter` — optional BPF filter expression to install on the capture handle.
///                    `Some(expr)` suppresses matching frames before forwarding;
///                    `None` forwards everything captured.
pub async fn run(
    pcap_device: String,
    local_addr: SocketAddr,
    remote_addr: SocketAddr,
    capture_filter: Option<String>,
) -> Result<()> {
    log::info!(
        "starting — iface={} local={} remote={}",
        pcap_device,
        local_addr,
        remote_addr
    );

    // Open the NIC for capture (promiscuous) and for injection as separate handles.
    // pcap::Capture<Active> is not Clone, so we open two independent handles to
    // the same device: one for reading frames, one for sending injected frames.
    let mut cap_handle = pcap::Capture::from_device(pcap_device.as_str())
        .with_context(|| format!("device '{}' not found", pcap_device))?
        .promisc(true)
        .snaplen(FRAME_BUF as i32)
        // Non-zero timeout allows the capture thread to check for shutdown.
        .timeout(100)
        .open()
        .context("failed to open pcap capture handle")?;

    // Install BPF filter if provided.  A filter is supplied when overlay and
    // underlay share the same NIC; omitting it in that case would cause a loop.
    match &capture_filter {
        Some(expr) => {
            log::info!("installing capture BPF filter: {}", expr);
            cap_handle
                .filter(expr, true)
                .context("failed to install BPF filter on capture handle")
                .map_err(|e| anyhow!(
                    "{e}\n[FATAL] Cannot install BPF filter on shared NIC. \
                     Running without the filter would create a forwarding loop. \
                     Use separate NICs for overlay and underlay, or fix the filter error above."
                ))?;
        }
        None => {
            log::info!("no capture BPF filter — overlay and underlay NICs are distinct");
        }
    }

    // The injection handle must be opened in promiscuous mode so that npcap
    // uses the raw-send path (PacketSendPacket via NDIS raw injection) rather
    // than the normal send path.  The normal path is subject to NDIS MAC
    // filtering: strict drivers (e.g. Intel) silently drop frames whose
    // destination MAC does not belong to the local NIC, including broadcasts.
    // Promiscuous mode lifts that restriction and allows arbitrary frames —
    // unicast, broadcast, and multicast — to be injected regardless of dst MAC.
    let inj_handle = pcap::Capture::from_device(pcap_device.as_str())
        .with_context(|| format!("device '{}' not found (inject)", pcap_device))?
        .promisc(true)
        .snaplen(FRAME_BUF as i32)
        .timeout(100)
        .open()
        .context("failed to open pcap injection handle")?;

    // Single shared UDP socket for both TX and RX.
    let udp = Arc::new(
        UdpSocket::bind(local_addr)
            .await
            .with_context(|| format!("UDP bind {} failed", local_addr))?,
    );
    log::info!("UDP socket bound on {}", local_addr);

    // ── Thread A: pcap capture → mpsc channel ─────────────────────────────
    // Runs in a blocking OS thread because pcap's next_packet() is blocking.
    let (cap_tx, mut cap_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);
    std::thread::Builder::new()
        .name("pcap-capture".into())
        .spawn(move || {
            let mut cap = cap_handle;
            loop {
                match cap.next_packet() {
                    Ok(pkt) => {
                        if cap_tx.blocking_send(pkt.data.to_vec()).is_err() {
                            // Receiver dropped; runtime is shutting down.
                            break;
                        }
                    }
                    Err(pcap::Error::TimeoutExpired) => continue,
                    Err(e) => {
                        log::error!("pcap capture: {e}");
                        break;
                    }
                }
            }
        })
        .context("failed to spawn capture thread")?;

    // ── Task A: mpsc channel → UDP TX ─────────────────────────────────────
    // `peer_unreachable`: set on WSAECONNRESET (either send or recv side).
    // Cleared only when Task B receives a real frame from the peer — that is
    // the only reliable signal that the peer port is open.
    let peer_unreachable = Arc::new(AtomicBool::new(false));
    let peer_unreachable_tx = peer_unreachable.clone();
    let udp_tx = udp.clone();
    let task_tx = tokio::spawn(async move {
        while let Some(frame) = cap_rx.recv().await {
            if let Err(e) = udp_tx.send_to(&frame, remote_addr).await {
                #[cfg(target_os = "windows")]
                if e.raw_os_error() == Some(WSAECONNRESET) {
                    if !peer_unreachable_tx.swap(true, Ordering::Relaxed) {
                        log::warn!(
                            "UDP send_to {}: ICMP port-unreachable received — \
                             remote IP is reachable but peer port is not open yet",
                            remote_addr
                        );
                    }
                    continue; // non-fatal: keep sending, peer may come up
                }
                log::error!("UDP send_to {}: {e}", remote_addr);
                break;
            }
        }
        log::warn!("capture→UDP task exiting");
    });

    // ── Thread B: mpsc channel → pcap inject ──────────────────────────────
    // Injection errors are non-fatal and logged at DEBUG level only.
    // See inline comment for details.
    let (inj_tx, mut inj_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);
    std::thread::Builder::new()
        .name("pcap-inject".into())
        .spawn(move || {
            let mut inj = inj_handle;
            while let Some(frame) = inj_rx.blocking_recv() {
                if let Err(e) = inj.sendpacket(frame) {
                    // Non-fatal: log at DEBUG level only.
                    // Error 31 (device not functioning) and error 20 (device not found)
                    // occur transiently when the NIC driver is busy or when a frame's
                    // destination MAC is unknown on the local segment — both are expected
                    // during tunnel bring-up and normal cross-segment operation.
                    log::error!("pcap inject: {e}");
                }
            }
        })
        .context("failed to spawn inject thread")?;

    // ── Task B: UDP RX → mpsc channel (→ inject thread) ───────────────────
    let udp_rx = udp.clone();
    let peer_unreachable_rx = peer_unreachable.clone();
    let task_rx = tokio::spawn(async move {
        let mut buf = vec![0u8; UDP_BUF];
        loop {
            match udp_rx.recv_from(&mut buf).await {
                Ok((n, src)) => {
                    // Real frame received — peer port is open; clear warning flag.
                    if peer_unreachable_rx.swap(false, Ordering::Relaxed) {
                        log::info!("UDP rx from {}: peer is now reachable", src);
                    }
                    log::debug!("UDP rx {} bytes from {}", n, src);
                    if inj_tx.send(buf[..n].to_vec()).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    // Windows WSAECONNRESET: remote returned ICMP Port Unreachable.
                    // Non-fatal — peer port not open yet; suppress duplicate warnings.
                    #[cfg(target_os = "windows")]
                    if e.raw_os_error() == Some(WSAECONNRESET) {
                        if !peer_unreachable_rx.swap(true, Ordering::Relaxed) {
                            log::warn!(
                                "UDP recv_from: ICMP port-unreachable from {} — \
                                 remote IP is reachable but peer port is not open yet",
                                remote_addr
                            );
                        }
                        continue;
                    }
                    log::error!("UDP recv_from: {e}");
                    break;
                }
            }
        }
        log::warn!("UDP→inject task exiting");
    });

    // ── Wait: any task exit or Ctrl+C ──────────────────────────────────────
    tokio::select! {
        _ = task_tx => { log::warn!("TX task ended"); }
        _ = task_rx => { log::warn!("RX task ended"); }
        _ = tokio::signal::ctrl_c() => {
            log::info!("Ctrl+C received, shutting down");
        }
    }

    Ok(())
}
