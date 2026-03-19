# L2Portal

Lightweight Layer-2 UDP tunnel bridge for Windows.  
Transparently bridges two Ethernet segments over a UDP tunnel — no encryption,
no handshake, bare Ethernet frames as UDP payload (wire-compatible with l2tunnel).

## Platform considerations

L2Portal is currently focused on Windows.

On Linux, similar functionality can already be achieved using native kernel
features such as VXLAN, bridge, and TAP/TUN devices, which provide a
well-integrated and high-performance solution for Layer-2 tunneling and LAN extension.

In contrast, Windows lacks a comparable set of composable Layer-2 primitives.
L2Portal is designed to fill this gap by providing a zero-configuration,
integrated solution built on top of npcap and TAP-Windows.

As a result, L2Portal prioritizes deep integration with the Windows networking
stack over cross-platform portability.

Future versions may include a Linux-based server component to support
heterogeneous deployments (e.g. Linux ↔ Windows), while keeping the client-side
experience on Windows simple and transparent.

## Modes

| Mode | Flag | Description |
|------|------|-------------|
| List interfaces | `--list` | Print all capturable interfaces and exit |
| Server | `--if <IFID>` | Capture a physical NIC and forward frames over UDP |
| Client | `--tap <NAME>[:<IP/prefix>]` | Create a TAP adapter, bridge to a UDP tunnel |

In typical deployments:

- L2Portal server mode replaces `l2tunnel + bridge`
- L2Portal client mode enables use cases that require direct interaction with applications

## Quick Start

```powershell
# List available interfaces
l2portal.exe --list

# Server mode (physical NIC bridge)
l2portal.exe --if "Ethernet" --local 0.0.0.0:4789 --remote 203.0.113.10:4789

# Client mode (TAP adapter, no static IP)
l2portal.exe --tap tap-ot --local 0.0.0.0:4789 --remote 203.0.113.1:4789

# Client mode (TAP adapter with static IP + automatic route injection)
l2portal.exe --tap tap-ot:192.168.10.50/24 --local 0.0.0.0:4789 --remote 203.0.113.1:4789
```

In client mode, the remote peer can be switched at runtime without restarting:

```
switch 203.0.113.20:4789
```

## Build

### Prerequisites

- Rust toolchain `stable-x86_64-pc-windows-gnu`
- npcap SDK extracted to `deps/npcap/sdk/`  
  Download: https://npcap.com/#download → "npcap-sdk-x.xx.zip"
- (For installer only) Inno Setup 6: https://jrsoftware.org/isinfo.php

### Compile

```powershell
# Set npcap SDK paths
$env:LIB     = "$PWD\deps\npcap\sdk\Lib\x64"
$env:INCLUDE = "$PWD\deps\npcap\sdk\Include"

cargo build --release --target x86_64-pc-windows-gnu
```

Or use the provided script (also compiles the installer):

```powershell
.\installer\build.ps1
```

### Required `deps/` layout (not tracked in git)

```
📂deps/
 ├─📂npcap/                   # from https://npcap.com/#download
 │  ├─📂installer/
 │  │  └─📄npcap-x.xx.exe
 │  └─📂sdk/
 │     ├─📂Include/
 │     └─📂Lib/
 └─📂tap/
    ├─📂amd64/                # from tap-windows6 dist.win10.zip
    │  ├─📄OemVista.inf
    │  ├─📄devcon.exe
    │  ├─📄tap0901.cat
    │  └─📄tap0901.sys
    └─📄tapctl.exe            # extracted from OpenVPN installer MSI
```

Sources:
- npcap: https://npcap.com/#download
- TAP-Windows6: https://github.com/OpenVPN/tap-windows6/releases
- tapctl.exe: extract from OpenVPN community installer MSI

## Runtime Requirements

At runtime, `l2portal.exe` requires:

- **npcap** installed (for server mode)
- **TAP-Windows6 driver** installed (for client mode)
- **tapctl.exe** in the same directory as `l2portal.exe`, or available on system PATH (for client mode)
- Administrator privileges (UAC prompt appears automatically on launch)

## Environment Variables

| Variable | Effect |
|----------|--------|
| `RUST_LOG` | Log level: `error`, `warn`, `info` (default), `debug` |

## Log Format

All log output goes to stderr, one line per message:

```
[INFO] server: UDP socket bound on 192.168.1.10:4789
[ERROR] tap: tapctl.exe not found in 'C:\Program Files\L2Portal'
```

## Installer

The Inno Setup script `installer/setup.iss` produces a single `.exe` installer that:

1. Detects and silently installs npcap (if not already installed)
2. Detects and silently installs TAP-Windows6 driver (if not already installed)
3. Copies `l2portal.exe`, `tapctl.exe`, and `devcon.exe` to `C:\Program Files\L2Portal\`
4. Adds the install directory to the system `PATH`

Uninstall removes only L2Portal files; npcap and TAP-Windows6 are left intact
(they may be shared by Wireshark, OpenVPN, etc.) unless the user explicitly
checks the optional removal boxes during uninstall.

## Security Note

This tool performs **no authentication and no encryption**.  
Do not expose the UDP port to untrusted networks.  
For secure deployments, run inside a VPN tunnel (WireGuard, OpenVPN, etc.).

## Comparison with l2tunnel

L2Portal is wire-compatible with [l2tunnel](https://github.com/tun2proxy/l2tunnel),
using the same bare Ethernet-over-UDP encapsulation format.

While l2tunnel focuses on providing a minimal Layer-2 transport between endpoints,
L2Portal extends this model by integrating the tunnel directly with the host
networking environment.

| Capability | l2tunnel | l2portal server | l2portal client |
|---|---|---|---|
| Inject frames into an **existing physical NIC** | ✗ — requires external bridging or forwarding logic | ✅ — frames are injected directly via npcap; no bridge required | — |
| Expose L2 traffic to **local applications** | ✗ — no built-in integration with the host networking stack | — | ✅ — frames are delivered through a TAP adapter visible to the OS and applications |
| Propagate frames onto the **local LAN** | ✗ — requires an external bridge (e.g. Linux bridge, Hyper-V switch) | ✅ — operates directly on the physical NIC in promiscuous mode | ✗ — by design; traffic terminates on the local machine |

### Design differences

- l2tunnel
  - Provides a minimal Layer-2 transport mechanism
  - The virtual interface acts as a tunnel endpoint only
  - Does not include built-in bridging or host stack integration
  - Intended to be combined with external components for full networking integration

- L2Portal (server mode)
  - Eliminates the need for an external bridge
  - Uses npcap to attach directly to an existing physical NIC
  - Preserves the NIC’s original IP configuration and normal traffic

- L2Portal (client mode)
  - Terminates the tunnel on a TAP adapter
  - Makes tunneled traffic available to the Windows TCP/IP stack
  - Allows unmodified applications to interact with the remote Layer-2 network

**In short:**

- l2tunnel: provides a transport primitive; cross-platform (e.g. Linux, Windows)
- L2Portal: provides an integrated solution; currently Windows-only
