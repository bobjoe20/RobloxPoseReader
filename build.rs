// Exports the pose block by name so an attached reader resolves it from the export
// table instead of depending on where a given build happens to place it.
//
// Also embeds proximity_companion.dll from the Proximity Core companion SDK, so
// the release is one exe. The exe carries the signed DLL byte for byte, and the
// build accepts only the one file whose hash is pinned here.

use sha2::{Digest, Sha256};
use std::path::PathBuf;

const SDK_URL: &str = "https://proximitycore.net/downloads/pxc-companion-sdk-0.5.0-windows-x64.zip";
const DLL_NAME: &str = "proximity_companion.dll";
const DLL_SHA256: &str = "6f1ab2a8a19f720c9a6e4fb1bd3eb0fbec0b6a08d4a7d043f9adbcd469decb86";

fn main() {
    println!("cargo:rustc-link-arg-bins=/EXPORT:PXC_POSE,DATA");
    embed_companion_dll();
}

fn embed_companion_dll() {
    println!("cargo:rerun-if-env-changed=PXC_COMPANION_SDK");
    let sdk = std::env::var_os("PXC_COMPANION_SDK").map_or_else(|| PathBuf::from("sdk"), PathBuf::from);
    let dll = sdk.join(DLL_NAME);
    println!("cargo:rerun-if-changed={}", dll.display());

    let bytes = std::fs::read(&dll).unwrap_or_else(|error| {
        panic!("{}: {error}. Unzip {SDK_URL} into sdk/.", dll.display())
    });
    let hash: String = Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect();
    assert!(
        hash == DLL_SHA256,
        "{} is not the signed DLL from {SDK_URL} (sha256 {hash})",
        dll.display()
    );

    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR")).join(DLL_NAME);
    std::fs::write(&out, &bytes).expect("OUT_DIR is writable");
    println!("cargo:rustc-env=PXC_COMPANION_DLL_SHA256={hash}");
}
