// build.rs — Embed UAC manifest and set npcap SDK library path at compile time.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        // Embed the UAC requireAdministrator manifest into the executable.
        let mut res = winres::WindowsResource::new();
        res.set_manifest_file("manifest.xml");
        res.compile().expect("[ERROR] build: failed to embed manifest.xml");

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
        println!("cargo:rerun-if-changed=manifest.xml");
        println!("cargo:rerun-if-changed=build.rs");
    }
}
