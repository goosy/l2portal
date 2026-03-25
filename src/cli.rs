// src/cli.rs — Command-line argument definitions using clap derive.
use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "l2portal",
    about = "Lightweight L2 UDP tunnel bridge",
    long_about = r#"Lightweight L2 UDP tunnel bridge.

Usage examples:
  l2portal.exe --list
  l2portal.exe --if "Ethernet" --local 0.0.0.0:4789 --remote 203.0.113.10:4789
  l2portal.exe --tap tap-ot --local 0.0.0.0:4789 --remote 203.0.113.1:4789
  l2portal.exe --tap tap-ot:192.168.10.50/24 --local 0.0.0.0:4789 --remote 203.0.113.1:4789
  l2portal.exe --tap auto --local 0.0.0.0:4789 --remote 203.0.113.1:4789
  l2portal.exe --tap auto:192.168.10.50/24 --local 0.0.0.0:4789 --remote 203.0.113.1:4789"#
)]
pub struct Args {
    /// List available capture interfaces and exit.
    #[arg(long, conflicts_with_all = ["iface", "tap", "local", "remote"])]
    pub list: bool,

    /// Server mode: physical interface to capture/inject.
    /// Accepts friendly name (e.g. "Ethernet"), ifIndex (e.g. "8"),
    /// or NPF GUID (e.g. "\Device\NPF_{...}").
    /// Note: `if` is a Rust keyword; mapped via `long = "if"`.
    #[arg(long = "if", value_name = "IFID")]
    pub iface: Option<String>,

    /// Client mode: TAP adapter name, optionally with static IP/prefix.
    /// Use "auto" to let tapctl auto-assign the adapter name.
    /// Format: <name> or <name>:<IP>/<prefix>  e.g. "tap-ot:192.168.10.50/24"
    #[arg(long, value_name = "NAME[:IP/PREFIX]")]
    pub tap: Option<String>,

    /// Local UDP bind address.  Use 0.0.0.0 to auto-detect via routing table.
    #[arg(long, value_name = "IP:PORT")]
    pub local: Option<String>,

    /// Remote UDP peer address (IP:PORT).
    #[arg(long, value_name = "IP:PORT")]
    pub remote: Option<String>,
}
