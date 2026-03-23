// src/iface.rs — Network interface enumeration, resolution, and --list output.
//
// Uses pcap to enumerate capturable interfaces, then cross-references
// Windows IP helper APIs to enrich with friendly name, ifIndex, and IP.

use anyhow::{anyhow, Context, Result};
use pcap::Device;
use std::collections::HashMap;
use std::net::Ipv4Addr;
use unicode_width::UnicodeWidthStr;

/// Metadata for a single network interface.
#[derive(Debug, Clone)]
pub struct IfaceInfo {
    /// npcap device name, e.g. "\Device\NPF_{GUID}"
    pub pcap_name: String,
    /// Windows-assigned interface index
    pub if_index: u32,
    /// Human-readable name shown in Network Settings
    pub friendly_name: String,
    /// Description string from the adapter
    pub description: String,
    /// First configured IPv4 address, if any
    pub ip: Option<Ipv4Addr>,
}

/// Enumerate all capturable interfaces and enrich with Windows metadata.
pub fn list_interfaces() -> Result<Vec<IfaceInfo>> {
    let devices = Device::list().context("pcap_findalldevs failed")?;

    // Build a lookup from GUID string to Windows adapter metadata.
    let win_map = build_windows_adapter_map()?;

    let mut result = Vec::new();
    for dev in devices {
        let pcap_name = dev.name.clone();
        // Extract GUID from "\Device\NPF_{GUID}" form.
        let guid = extract_guid(&pcap_name).unwrap_or_default().to_uppercase();

        if let Some(meta) = win_map.get(&guid) {
            result.push(IfaceInfo {
                pcap_name,
                if_index: meta.if_index,
                friendly_name: meta.friendly_name.clone(),
                description: meta.description.clone(),
                ip: meta.ip,
            });
        } else {
            // Fallback: use npcap description if Windows cross-reference fails.
            let desc = dev.desc.unwrap_or_default();
            result.push(IfaceInfo {
                pcap_name,
                if_index: 0,
                friendly_name: String::new(),
                description: desc,
                ip: None,
            });
        }
    }
    Ok(result)
}

/// Print the interface list to stdout in a fixed-width table.
///
/// Column order: ifIdx | Name (28) | IP (15) | Description (truncated, last)
/// Uses Unicode display width so CJK characters (width=2) align correctly.
pub fn print_interface_list(ifaces: &[IfaceInfo]) {
    const NAME_W: usize = 30;
    const IP_W:   usize = 15;
    const DESC_W: usize = 45;

    println!(
        "  {:>6}  {}  {}  {}",
        "ifIdx",
        fit_str("Name", NAME_W),
        fit_str("IP", IP_W),
        "Description",
    );
    println!(
        "  {:->6}  {}  {}  {}",
        "",
        "-".repeat(NAME_W),
        "-".repeat(IP_W),
        "-".repeat(DESC_W),
    );
    for iface in ifaces {
        let ip_str = iface
            .ip
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "-".to_string());

        let name_src = if iface.friendly_name.is_empty() {
            &iface.description
        } else {
            &iface.friendly_name
        };

        println!(
            "  {:>6}  {}  {}  {}",
            iface.if_index,
            fit_str(name_src, NAME_W),
            fit_str(&ip_str, IP_W),
            truncate_str(&iface.description, DESC_W),
        );
    }
}

/// Pad or truncate `s` to exactly `col_width` display columns.
/// CJK and other wide characters count as 2 columns each.
/// Truncation appends '…' and pads the remainder with spaces.
fn fit_str(s: &str, col_width: usize) -> String {
    let display_w = s.width();
    if display_w <= col_width {
        // Pad with spaces to reach col_width.
        format!("{}{}", s, " ".repeat(col_width - display_w))
    } else {
        truncate_str(s, col_width)
    }
}

/// Truncate `s` so its display width is at most `col_width`, appending '…'.
/// The result is NOT padded — use `fit_str` when padding is needed.
fn truncate_str(s: &str, col_width: usize) -> String {
    if s.width() <= col_width {
        return s.to_string();
    }
    // Reserve 1 display column for '…' (itself width=1).
    let budget = col_width.saturating_sub(1);
    let mut result = String::new();
    let mut used = 0usize;
    for ch in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + cw > budget {
            break;
        }
        result.push(ch);
        used += cw;
    }
    result.push('…');
    result
}

/// Look up an `IfaceInfo` by its exact pcap device name.
///
/// Used by server mode to cross-reference the resolved pcap device back to
/// interface metadata (e.g. IP address) for underlay-overlap detection.
pub fn find_iface_by_pcap_name<'a>(pcap_name: &str, ifaces: &'a [IfaceInfo]) -> Option<&'a IfaceInfo> {
    ifaces
        .iter()
        .find(|i| i.pcap_name.eq_ignore_ascii_case(pcap_name))
}

/// Resolve a user-supplied `--if` value to a pcap device name.
///
/// Accepted forms (tried in order):
///   1. Exact NPF path  ("\Device\NPF_{...}")
///   2. Numeric ifIndex
///   3. Friendly name (case-insensitive)
pub fn resolve_iface(user_input: &str, ifaces: &[IfaceInfo]) -> Result<String> {
    // 1. NPF path — use directly if npcap knows it.
    if user_input.starts_with(r"\Device\NPF_") {
        let found = ifaces
            .iter()
            .any(|i| i.pcap_name.eq_ignore_ascii_case(user_input));
        if found {
            return Ok(user_input.to_string());
        }
        return Err(anyhow!(
            "[ERROR] iface: NPF device '{}' not found in pcap device list",
            user_input
        ));
    }

    // 2. Numeric ifIndex.
    if let Ok(idx) = user_input.parse::<u32>() {
        if let Some(iface) = ifaces.iter().find(|i| i.if_index == idx) {
            return Ok(iface.pcap_name.clone());
        }
        return Err(anyhow!(
            "[ERROR] iface: no interface with ifIndex {} found",
            idx
        ));
    }

    // 3. Friendly name (case-insensitive).
    let lower = user_input.to_lowercase();
    if let Some(iface) = ifaces
        .iter()
        .find(|i| i.friendly_name.to_lowercase() == lower)
    {
        return Ok(iface.pcap_name.clone());
    }

    Err(anyhow!(
        "[ERROR] iface: '{}' did not match any interface by name or index. \
         Run --list to see available interfaces.",
        user_input
    ))
}

// ─── Windows helpers ──────────────────────────────────────────────────────────

struct WinAdapterMeta {
    if_index: u32,
    friendly_name: String,
    description: String,
    ip: Option<Ipv4Addr>,
}

/// Build a map from uppercase GUID string → Windows adapter metadata.
///
/// Uses GetAdaptersInfo (IPv4, broadly compatible) to enumerate adapters, then
/// ConvertInterfaceIndexToLuid + ConvertInterfaceLuidToAlias to get friendly names.
///
/// IP_ADAPTER_INFO fields used (windows-sys 0.59 names):
///   .Next           *mut IP_ADAPTER_INFO — linked list
///   .Index          u32                  — IPv4 ifIndex for the adapter
///   .AdapterName    [u8; MAX_ADAPTER_NAME_LENGTH+4]  — GUID string
///   .Description    [u8; MAX_ADAPTER_DESCRIPTION_LENGTH+4]
///   .IpAddressList.IpAddress.String  [u8; 16]
#[cfg(target_os = "windows")]
fn build_windows_adapter_map() -> Result<HashMap<String, WinAdapterMeta>> {
    use windows_sys::Win32::Foundation::ERROR_BUFFER_OVERFLOW;
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetAdaptersInfo, IP_ADAPTER_INFO,
    };

    let mut map = HashMap::new();
    let mut buf_len: u32 = 0;

    // First call with null buffer: Windows returns the required size.
    let ret = unsafe { GetAdaptersInfo(std::ptr::null_mut(), &mut buf_len) };
    if ret != 0 && ret != ERROR_BUFFER_OVERFLOW {
        log::warn!("GetAdaptersInfo size probe failed: {ret}");
        return Ok(map);
    }

    // Allocate a byte buffer aligned to IP_ADAPTER_INFO.
    let size = std::mem::size_of::<IP_ADAPTER_INFO>();
    // Round up buf_len to a multiple of the struct size, then add one extra.
    let capacity = (buf_len as usize + size - 1) / size + 1;
    let mut raw: Vec<IP_ADAPTER_INFO> = vec![unsafe { std::mem::zeroed() }; capacity];
    let mut out_len = (capacity * size) as u32;

    let ret = unsafe { GetAdaptersInfo(raw.as_mut_ptr(), &mut out_len) };
    if ret != 0 {
        log::warn!("GetAdaptersInfo failed: {ret}");
        return Ok(map);
    }

    // Walk the linked list.
    let mut ptr: *const IP_ADAPTER_INFO = raw.as_ptr();
    while !ptr.is_null() {
        let adapter = unsafe { &*ptr };

        // AdapterName holds the GUID string (with or without braces).
        let adapter_name = c_bytes_to_string(&adapter.AdapterName);
        let guid = adapter_name
            .trim_matches(|c: char| c == '{' || c == '}')
            .to_uppercase();

        let description = c_bytes_to_string(&adapter.Description);

        // First IP address from the linked IP list.
        let ip_str = c_bytes_to_string(&adapter.IpAddressList.IpAddress.String);
        let ip = ip_str
            .parse::<Ipv4Addr>()
            .ok()
            .filter(|ip| !ip.is_unspecified());

        // Index is the adapter's IPv4 interface index.
        let if_index = adapter.Index;

        let friendly_name =
            get_friendly_name(if_index).unwrap_or_else(|| description.clone());

        map.insert(
            guid,
            WinAdapterMeta {
                if_index,
                friendly_name,
                description,
                ip,
            },
        );

        ptr = adapter.Next;
    }
    Ok(map)
}

#[cfg(not(target_os = "windows"))]
fn build_windows_adapter_map() -> Result<HashMap<String, WinAdapterMeta>> {
    Ok(HashMap::new())
}

/// Retrieve the friendly (display) name for a network adapter by ifIndex.
///
/// Uses ConvertInterfaceIndexToLuid then ConvertInterfaceLuidToAlias.
/// The alias buffer is 257 UTF-16 code units (IF_MAX_STRING_SIZE + 1).
#[cfg(target_os = "windows")]
fn get_friendly_name(if_index: u32) -> Option<String> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        ConvertInterfaceIndexToLuid, ConvertInterfaceLuidToAlias,
    };
    use windows_sys::Win32::NetworkManagement::Ndis::NET_LUID_LH;

    unsafe {
        let mut luid: NET_LUID_LH = std::mem::zeroed();
        if ConvertInterfaceIndexToLuid(if_index, &mut luid) != 0 {
            return None;
        }
        // IF_MAX_STRING_SIZE = 256; allocate 257 for the null terminator.
        let mut alias = [0u16; 257];
        if ConvertInterfaceLuidToAlias(&luid, alias.as_mut_ptr(), alias.len()) != 0 {
            return None;
        }
        let end = alias.iter().position(|&c| c == 0).unwrap_or(alias.len());
        Some(String::from_utf16_lossy(&alias[..end]))
    }
}

#[cfg(not(target_os = "windows"))]
fn get_friendly_name(_if_index: u32) -> Option<String> {
    None
}

/// Extract the GUID portion from "\Device\NPF_{GUID}".
fn extract_guid(npf_name: &str) -> Option<&str> {
    // Find the opening brace.
    let start = npf_name.find('{')? + 1;
    let end = npf_name.find('}')?;
    if start < end {
        Some(&npf_name[start..end])
    } else {
        None
    }
}

/// Convert a null-terminated C byte array to a Rust String.
fn c_bytes_to_string(bytes: &[i8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let u8_slice: Vec<u8> = bytes[..end].iter().map(|&b| b as u8).collect();
    String::from_utf8_lossy(&u8_slice).into_owned()
}
