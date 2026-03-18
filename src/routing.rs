// src/routing.rs — Host route injection/removal and best-route lookup.
//
// Provides:
//   - resolve_local_ip()  : auto-detect local bind IP via GetBestRoute2
//   - route_add_host()    : inject /32 host route via `route add`
//   - route_delete_host() : remove /32 host route via `route delete`
//   - get_best_route()    : query outbound gateway+ifIndex for a destination IP

use anyhow::{anyhow, Result};
use std::net::Ipv4Addr;
use std::process::Command;

/// Query result from GetBestRoute2.
#[derive(Debug, Clone)]
pub struct BestRoute {
    /// Next-hop gateway IP.
    pub gateway: Ipv4Addr,
    /// Interface index for the outbound path.
    pub if_index: u32,
    /// Local address of the outbound interface.
    pub local_ip: Ipv4Addr,
}

/// Resolve the effective local bind address.
///
/// If `local_ip` is `0.0.0.0`, queries the routing table for `dest_ip`
/// and returns the local address of the outbound interface.
/// Otherwise returns `local_ip` unchanged.
pub fn resolve_local_ip(local_ip: Ipv4Addr, dest_ip: Ipv4Addr) -> Result<Ipv4Addr> {
    if !local_ip.is_unspecified() {
        return Ok(local_ip);
    }
    let route = get_best_route(dest_ip)?;
    log::info!(
        "auto-detected local bind IP {} (via ifIndex {}, gw {})",
        route.local_ip,
        route.if_index,
        route.gateway
    );
    Ok(route.local_ip)
}

/// Query the Windows routing table for the best route to `dest`.
///
/// Uses GetBestRoute2 which fills in the best MIB_IPFORWARD_ROW2 and the
/// source address on the outbound interface.
///
/// SOCKADDR_INET layout (for IPv4 path):
///   si_family (u16) + Ipv4 (SOCKADDR_IN: sin_family u16, sin_port u16, sin_addr IN_ADDR [u8;4], ...)
///   We access sin_addr as a [u8; 4] via the S_un.S_un_b.s_b1..4 byte fields,
///   or simply via S_un.S_addr as a u32 in network byte order.
#[cfg(target_os = "windows")]
pub fn get_best_route(dest: Ipv4Addr) -> Result<BestRoute> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetBestRoute2, MIB_IPFORWARD_ROW2,
    };
    use windows_sys::Win32::Networking::WinSock::{AF_INET, SOCKADDR_INET};

    unsafe {
        // Build the destination SOCKADDR_INET.
        let mut dest_addr: SOCKADDR_INET = std::mem::zeroed();
        // si_family sits at byte 0, size 2.
        dest_addr.si_family = AF_INET as u16;
        // Ipv4.sin_addr.S_un.S_addr — store as little-endian u32 of the
        // IPv4 address bytes (network byte order = big endian).
        let dest_octets = dest.octets();
        // Write the four address bytes into the sin_addr position.
        // SOCKADDR_INET is a union; Ipv4 starts at offset 0.
        // sin_addr (IN_ADDR) starts at offset 4 within SOCKADDR_IN.
        // We use a raw pointer write to avoid relying on brittle field paths.
        let base = &mut dest_addr as *mut SOCKADDR_INET as *mut u8;
        // offset 0: si_family (already set above via union field)
        // For the Ipv4 union member (SOCKADDR_IN):
        //   offset 0: sin_family  (u16)
        //   offset 2: sin_port    (u16)
        //   offset 4: sin_addr    (IN_ADDR = [u8;4])
        base.add(4).copy_from_nonoverlapping(dest_octets.as_ptr(), 4);

        let mut best_source: SOCKADDR_INET = std::mem::zeroed();
        let mut row: MIB_IPFORWARD_ROW2 = std::mem::zeroed();

        let ret = GetBestRoute2(
            std::ptr::null(),   // InterfaceLuid (null = any)
            0,                  // InterfaceIndex (0 = any)
            std::ptr::null(),   // SourceAddress (null = any)
            &dest_addr,
            0,                  // AddressSortOptions
            &mut row,
            &mut best_source,
        );
        if ret != 0 {
            return Err(anyhow!(
                "[ERROR] routing: GetBestRoute2 failed with code {:#x}",
                ret
            ));
        }

        // Extract gateway from row.NextHop (SOCKADDR_INET, same layout).
        let nhop_base = &row.NextHop as *const SOCKADDR_INET as *const u8;
        let mut gw_bytes = [0u8; 4];
        nhop_base.add(4).copy_to_nonoverlapping(gw_bytes.as_mut_ptr(), 4);
        let gateway = Ipv4Addr::from(gw_bytes);

        // Extract local source address from best_source.
        let src_base = &best_source as *const SOCKADDR_INET as *const u8;
        let mut src_bytes = [0u8; 4];
        src_base.add(4).copy_to_nonoverlapping(src_bytes.as_mut_ptr(), 4);
        let local_ip = Ipv4Addr::from(src_bytes);

        let if_index = row.InterfaceIndex;

        // On-link routes have gateway = 0.0.0.0; use dest itself as the
        // next-hop for `route add` (Windows convention for on-link routes).
        let gateway = if gateway.is_unspecified() { dest } else { gateway };

        Ok(BestRoute { gateway, if_index, local_ip })
    }
}

#[cfg(not(target_os = "windows"))]
pub fn get_best_route(_dest: Ipv4Addr) -> Result<BestRoute> {
    Err(anyhow!("[ERROR] routing: get_best_route not implemented on this platform"))
}

/// Inject a /32 host route via the Windows `route` command.
///
/// This must be called **before** `tap_set_ip` to prevent TAP from
/// intercepting the underlay UDP traffic.
pub fn route_add_host(remote_ip: Ipv4Addr, gateway: Ipv4Addr, if_idx: u32) -> Result<()> {
    log::info!(
        "adding host route {} via {} (ifIdx {})",
        remote_ip, gateway, if_idx
    );
    let status = Command::new("route")
        .args([
            "add",
            &remote_ip.to_string(),
            "mask",
            "255.255.255.255",
            &gateway.to_string(),
            "if",
            &if_idx.to_string(),
        ])
        .status()
        .map_err(|e| anyhow!("[ERROR] routing: failed to run route.exe: {e}"))?;
    if !status.success() {
        return Err(anyhow!(
            "[ERROR] routing: route add host {} failed: {}",
            remote_ip,
            status
        ));
    }
    Ok(())
}

/// Remove a /32 host route previously added by `route_add_host`.
pub fn route_delete_host(remote_ip: Ipv4Addr) -> Result<()> {
    log::info!("deleting host route {}", remote_ip);
    let _ = Command::new("route")
        .args(["delete", &remote_ip.to_string(), "mask", "255.255.255.255"])
        .status();
    Ok(())
}

/// Convert a prefix length to a dotted-decimal subnet mask.
pub fn prefix_to_mask(prefix: u8) -> Ipv4Addr {
    if prefix == 0 {
        return Ipv4Addr::new(0, 0, 0, 0);
    }
    let bits = !0u32 << (32 - prefix);
    Ipv4Addr::from(bits)
}
