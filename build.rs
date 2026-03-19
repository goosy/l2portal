// build.rs — Embed UAC manifest and set npcap SDK library path at compile time.
//
// Uses embed-resource instead of winres: winres requires rc.exe (MSVC only),
// embed-resource supports both MSVC and GNU toolchains (stable-x86_64-pc-windows-gnu).
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        // Embed the UAC requireAdministrator manifest into the executable.
        // manifest.rc references manifest.xml as RT_MANIFEST resource ID 1.
        embed_resource::compile("manifest.rc", embed_resource::NONE);

        // Point the linker to the npcap SDK static libraries.
        // Expected layout: deps/npcap/sdk/Lib/x64/
        // Can be overridden by setting the LIB environment variable externally.
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let npcap_lib = std::path::Path::new(&manifest_dir)
            .join("deps")
            .join("npcap")
            .join("sdk")
            .join("Lib")
            .join("x64");
        if npcap_lib.exists() {
            println!("cargo:rustc-link-search=native={}", npcap_lib.display());
        }
        // Link against wpcap (npcap) and the Windows Packet library.
        println!("cargo:rustc-link-lib=wpcap");
        println!("cargo:rustc-link-lib=Packet");

        // Re-run build script only when these files change.
        println!("cargo:rerun-if-changed=manifest.rc");
        println!("cargo:rerun-if-changed=manifest.xml");
        println!("cargo:rerun-if-changed=build.rs");
    }
}
