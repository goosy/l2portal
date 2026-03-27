// src/pcap.rs — FFI binding for pcap_init(PCAP_CHAR_ENC_UTF_8).
//
// Problem
// -------
// The `pcap` crate (v2) does not expose any API to call `pcap_init()`.
// npcap ≥ 1.00 ships libpcap 1.9.0 which introduced `pcap_init(opts, errbuf)`;
// when called with `PCAP_CHAR_ENC_UTF_8` it guarantees that every subsequent
// error string returned by the library is valid UTF-8.  Without this call,
// error strings on Windows are in the system ANSI code page (e.g. CP950 on
// Traditional-Chinese Windows), and the `pcap` crate's `CStr::to_str()`
// conversion fails with:
//
//   [ERROR] server: pcap inject: libpcap returned invalid UTF-8:
//           invalid utf-8 sequence of 1 bytes from index 41
//
// Fix
// ---
// Call `pcap_init(PCAP_CHAR_ENC_UTF_8, errbuf)` once at program startup,
// before any other pcap API.  We declare the symbol via `extern "C"` and
// link against `wpcap` (already listed in build.rs).
//
// Minimum version requirement
// ---------------------------
// Requires npcap ≥ 1.00.  This is already the project's stated minimum
// (see docs/design/L2Portal-design.md, Third-party dependencies).
// No runtime version check is performed; if a user installs an older npcap
// the binary will fail to load due to the missing `pcap_init` export, which
// is an acceptable hard error given the documented requirement.
//
// Thread safety
// -------------
// `pcap_init` must be called before any other libpcap function and is not
// thread-safe with respect to concurrent pcap calls.  Calling it from
// `main()` before the Tokio runtime starts satisfies this requirement.

/// Error buffer size required by the pcap API (`PCAP_ERRBUF_SIZE` = 256).
const PCAP_ERRBUF_SIZE: usize = 256;

/// `PCAP_CHAR_ENC_UTF_8` — instruct libpcap to encode all strings as UTF-8.
/// Defined in pcap/pcap.h as 0x00000001.
const PCAP_CHAR_ENC_UTF_8: u32 = 0x00000001;

unsafe extern "C" {
    /// `int pcap_init(unsigned int opts, char *errbuf)`
    ///
    /// Introduced in libpcap 1.9.0 / npcap 1.00.
    /// Returns 0 on success, -1 on error (message written to `errbuf`).
    fn pcap_init(opts: u32, errbuf: *mut std::ffi::c_char) -> std::ffi::c_int;
}

/// Call `pcap_init(PCAP_CHAR_ENC_UTF_8)` so that all subsequent libpcap
/// error strings are guaranteed to be UTF-8.
///
/// Must be called once, before any other pcap API, from a single thread.
///
/// Returns `Err` only if `pcap_init` itself reports an error, which in
/// practice should never happen.
pub fn init_utf8_encoding() -> Result<(), String> {
    let mut errbuf = [0i8; PCAP_ERRBUF_SIZE];
    let rc = unsafe { pcap_init(PCAP_CHAR_ENC_UTF_8, errbuf.as_mut_ptr()) };
    if rc != 0 {
        let msg = unsafe {
            std::ffi::CStr::from_ptr(errbuf.as_ptr())
                .to_string_lossy()
                .into_owned()
        };
        return Err(format!("pcap_init failed: {}", msg));
    }
    Ok(())
}
