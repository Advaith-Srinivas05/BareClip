use std::{env, path::Path};

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("cargo sets the manifest folder");
    let icon = Path::new(&manifest_dir).join("../../assets/brand/windows/bareclip.ico");
    println!("cargo:rerun-if-changed={}", icon.display());

    winresource::WindowsResource::new()
        .set_icon(icon.to_str().expect("the icon path is valid text"))
        .compile()
        .expect("the Windows icon compiles");
}
