// Exports the pose block by name so an attached reader resolves it from the export
// table instead of depending on where a given build happens to place it.
fn main() {
    println!("cargo:rustc-link-arg-bins=/EXPORT:PXC_POSE,DATA");
}
