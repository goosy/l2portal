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

use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;

/// Ethernet frame buffer size: standard MTU 1500 + header 14 + FCS 4 = 1518.
const FRAME_BUF: usize = 1518;

/// UDP receive buffer: slightly larger than max frame to detect oversized packets.
const UDP_BUF: usize = 1600;

/// Run the server mode event loop.
///
/// `pcap_device` — npcap device path resolved by `iface::resolve_iface`.
/// `local_addr`  — UDP socket bind address (already resolved from routing if 0.0.0.0).
/// `remote_addr` — fixed remote peer address.
pub async fn run(pcap_device: String, local_addr: SocketAddr, remote_addr: SocketAddr) -> Result<()> {
    log::info!(
        "starting — iface={} local={} remote={}",
        pcap_device,
        local_addr,
        remote_addr
    );

    // Open the NIC for capture (promiscuous) and for injection as separate handles.
    // pcap::Capture<Active> is not Clone, so we open two independent handles to
    // the same device: one for reading frames, one for sending injected frames.
    let cap_handle = pcap::Capture::from_device(pcap_device.as_str())
        .with_context(|| format!("device '{}' not found", pcap_device))?
        .promisc(true)
        .snaplen(FRAME_BUF as i32)
        // Non-zero timeout allows the capture thread to check for shutdown.
        .timeout(100)
        .open()
        .context("failed to open pcap capture handle")?;

    let inj_handle = pcap::Capture::from_device(pcap_device.as_str())
        .with_context(|| format!("device '{}' not found (inject)", pcap_device))?
        .promisc(false)
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
                        eprintln!("pcap capture: {e}");
                        break;
                    }
                }
            }
        })
        .context("failed to spawn capture thread")?;

    // ── Task A: mpsc channel → UDP TX ─────────────────────────────────────
    let udp_tx = udp.clone();
    let task_tx = tokio::spawn(async move {
        while let Some(frame) = cap_rx.recv().await {
            if let Err(e) = udp_tx.send_to(&frame, remote_addr).await {
                log::error!("UDP send_to {}: {e}", remote_addr);
                break;
            }
        }
        log::warn!("capture→UDP task exiting");
    });

    // ── Thread B: mpsc channel → pcap inject ──────────────────────────────
    let (inj_tx, mut inj_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);
    std::thread::Builder::new()
        .name("pcap-inject".into())
        .spawn(move || {
            let mut inj = inj_handle;
            while let Some(frame) = inj_rx.blocking_recv() {
                if let Err(e) = inj.sendpacket(frame) {
                    eprintln!("pcap inject: {e}");
                    // Injection errors are non-fatal; keep running.
                }
            }
        })
        .context("failed to spawn inject thread")?;

    // ── Task B: UDP RX → mpsc channel (→ inject thread) ───────────────────
    let udp_rx = udp.clone();
    let task_rx = tokio::spawn(async move {
        let mut buf = vec![0u8; UDP_BUF];
        loop {
            match udp_rx.recv_from(&mut buf).await {
                Ok((n, src)) => {
                    log::debug!("UDP rx {} bytes from {}", n, src);
                    if inj_tx.send(buf[..n].to_vec()).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
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
