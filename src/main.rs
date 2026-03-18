// src/main.rs — l2portal entry point.
//
// Dispatches to one of three modes based on CLI arguments:
//   --list          : enumerate capturable interfaces and exit
//   --if <IFID>     : server mode (physical NIC ↔ UDP tunnel)
//   --tap <TAP>     : client mode (TAP virtual NIC ↔ UDP tunnel)
//
// All error messages are written to stderr in the format:
//   [ERROR] <module>: <message>

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
    name: String,
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
            name: name.to_string(),
            ip_prefix: Some((ip, prefix)),
        })
    } else {
        Ok(TapArg {
            name: s.to_string(),
            ip_prefix: None,
        })
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
            log::info!(
                "server mode — pcap={} local={} remote={}",
                pcap_device,
                local_addr,
                remote_addr
            );
            server::run(pcap_device, local_addr, remote_addr).await?;
        }

        // Client mode: --tap
        (None, Some(tap_str)) => {
            let tap_arg = parse_tap_arg(tap_str)?;
            let params = client::ClientParams {
                tap_name: tap_arg.name,
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
