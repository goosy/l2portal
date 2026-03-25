// src/main.rs — l2portal entry point.
//
// Dispatches to one of three modes based on CLI arguments:
//   --list          : enumerate capturable interfaces and exit
//   --if <IFID>     : server mode (physical NIC ↔ UDP tunnel)
//   --tap <TAP>     : client mode (TAP virtual NIC ↔ UDP tunnel)
//
// All error messages are written to stderr in the format:
//   [ERROR] <module>: <message>
//
// Capture BPF filter — server mode:
//   server::run accepts an `Option<String>` capture filter that is built here
//   and passed in, keeping filter policy out of the server loop.  Currently
//   the only implemented use is same-NIC loop prevention (described below);
//   other filtering needs can be added here without touching server::run.
//
//   Same-NIC loop prevention:
//   When the overlay NIC (--if) and the underlay NIC (UDP egress to --remote)
//   are the same physical adapter, the tunnel's own UDP packets would be
//   re-captured and re-forwarded, creating an infinite loop.
//
//   Detection uses Windows interface indices (if_index), which is reliable
//   even when an adapter carries multiple IP addresses.
//
//   When same-NIC is confirmed, a narrow BPF filter is built and passed to
//   server::run.  The filter matches only the tunnel session's own packets,
//   keyed on the remote endpoint, so unrelated traffic from other hosts on
//   the same port is not affected:
//
//     not (udp and ((src host R and src port RP) or (dst host R and dst port RP)))
//
//   When the two NICs are distinct, `capture_filter` is None.

mod cli;
mod client;
mod iface;
mod logger;
mod routing;
mod server;
mod state;
mod tap;

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use std::net::{Ipv4Addr, SocketAddr};

/// TAP argument: name and optional IP/prefix.
struct TapArg {
    name: Option<String>,
    ip_prefix: Option<(Ipv4Addr, u8)>,
}

fn parse_tap_arg(s: &str) -> Result<TapArg> {
    if let Some((name, addr_str)) = s.split_once(':') {
        let (ip_str, prefix_str) = addr_str.split_once('/').ok_or_else(|| {
            anyhow!("[ERROR] main: --tap IP/prefix format must be IP/prefix, e.g. 192.168.10.50/24")
        })?;
        let ip: Ipv4Addr = ip_str
            .parse()
            .with_context(|| format!("[ERROR] main: invalid TAP IP '{}'", ip_str))?;
        let prefix: u8 = prefix_str
            .parse()
            .with_context(|| format!("[ERROR] main: invalid prefix '{}'", prefix_str))?;
        if prefix > 32 {
            return Err(anyhow!("[ERROR] main: prefix {} exceeds 32", prefix));
        }
        Ok(TapArg {
            name: parse_tap_name(name),
            ip_prefix: Some((ip, prefix)),
        })
    } else {
        Ok(TapArg {
            name: parse_tap_name(s),
            ip_prefix: None,
        })
    }
}

fn parse_tap_name(s: &str) -> Option<String> {
    if s.eq_ignore_ascii_case("auto") {
        None
    } else {
        Some(s.to_string())
    }
}

/// Parse and validate a `LocalIP:PORT` string.
/// Returns the socket address (still may be 0.0.0.0 if the user typed that).
fn parse_local(s: &str) -> Result<SocketAddr> {
    s.parse::<SocketAddr>()
        .with_context(|| format!("[ERROR] main: invalid --local address '{}'", s))
}

/// Parse a `RemoteIP:PORT` string.
fn parse_remote(s: &str) -> Result<SocketAddr> {
    s.parse::<SocketAddr>()
        .with_context(|| format!("[ERROR] main: invalid --remote address '{}'", s))
}

#[tokio::main]
async fn main() {
    // Initialize structured logger before anything else.
    logger::init();

    if let Err(e) = run().await {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let args = cli::Args::parse();

    // ── --list ────────────────────────────────────────────────────────────
    if args.list {
        let ifaces = iface::list_interfaces()
            .context("[ERROR] main: failed to enumerate interfaces")?;
        iface::print_interface_list(&ifaces);
        return Ok(());
    }

    // ── Require --local and --remote for all tunnel modes ─────────────────
    let local_str = args
        .local
        .as_deref()
        .ok_or_else(|| anyhow!("[ERROR] main: --local is required"))?;
    let remote_str = args
        .remote
        .as_deref()
        .ok_or_else(|| anyhow!("[ERROR] main: --remote is required"))?;

    let local_raw = parse_local(local_str)?;
    let remote_addr = parse_remote(remote_str)?;

    // Resolve 0.0.0.0 to the actual outbound interface address.
    let local_ip = match local_raw.ip() {
        std::net::IpAddr::V4(ip) => ip,
        _ => return Err(anyhow!("[ERROR] main: --local must be an IPv4 address")),
    };
    let remote_ip = match remote_addr.ip() {
        std::net::IpAddr::V4(ip) => ip,
        _ => return Err(anyhow!("[ERROR] main: --remote must be an IPv4 address")),
    };

    let resolved_local_ip = routing::resolve_local_ip(local_ip, remote_ip)
        .context("[ERROR] main: cannot resolve local bind address")?;
    let local_addr = SocketAddr::new(
        std::net::IpAddr::V4(resolved_local_ip),
        local_raw.port(),
    );

    // ── Mode dispatch ─────────────────────────────────────────────────────
    match (&args.iface, &args.tap) {
        // Server mode: --if
        (Some(iface_input), None) => {
            let ifaces = iface::list_interfaces()
                .context("[ERROR] main: interface enumeration failed")?;
            let pcap_device = iface::resolve_iface(iface_input, &ifaces)
                .context("[ERROR] main: cannot resolve --if argument")?;

            // Build capture_filter for same-NIC loop prevention.
            // See the file-level comment for rationale, detection method, and
            // filter expression design.
            //
            // Note: get_best_route is called a second time here — resolve_local_ip
            // already called it internally, but threading the if_index through
            // that return type is not worth the added complexity.
            let capture_filter: Option<String> = (|| {
                let underlay_if_index = routing::get_best_route(remote_ip)
                    .map(|r| r.if_index)
                    .unwrap_or_else(|e| {
                        log::warn!(
                            "could not resolve underlay if_index: {e} — \
                             same-NIC BPF check skipped"
                        );
                        0
                    });
                let cap_if_index = iface::find_iface_by_pcap_name(&pcap_device, &ifaces)
                    .map(|i| i.if_index)
                    .unwrap_or_else(|| {
                        log::warn!(
                            "could not resolve if_index for capture device '{}' — \
                             same-NIC BPF check skipped",
                            pcap_device
                        );
                        0
                    });

                if underlay_if_index == 0 || cap_if_index == 0 {
                    return None;
                }
                if cap_if_index != underlay_if_index {
                    return None;
                }

                // Same NIC confirmed.  remote_ip is already Ipv4Addr (validated
                // above), so no further matching is needed.
                let rp = remote_addr.port();
                Some(format!(
                    "not (udp and \
                     ((src host {rip} and src port {rp}) or \
                      (dst host {rip} and dst port {rp})))",
                    rip = remote_ip,
                    rp  = rp,
                ))
            })();

            log::info!(
                "server mode — pcap={} local={} remote={} bpf={}",
                pcap_device,
                local_addr,
                remote_addr,
                capture_filter.as_deref().unwrap_or("none"),
            );
            server::run(pcap_device, local_addr, remote_addr, capture_filter).await?;
        }

        // Client mode: --tap
        (None, Some(tap_str)) => {
            let tap_arg = parse_tap_arg(tap_str)?;
            let params = client::ClientParams {
                tap_name: tap_arg.name.unwrap_or_else(|| "auto".to_string()),
                tap_ip_prefix: tap_arg.ip_prefix,
                local_addr,
                remote_addr,
            };
            client::run(params).await?;
        }

        // Conflict: both or neither specified.
        (Some(_), Some(_)) => {
            return Err(anyhow!(
                "[ERROR] main: --if and --tap are mutually exclusive"
            ));
        }
        (None, None) => {
            return Err(anyhow!(
                "[ERROR] main: specify --if (server mode), --tap (client mode), or --list"
            ));
        }
    }

    Ok(())
}
