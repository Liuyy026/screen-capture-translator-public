use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let native = std::env::var_os("KOMA_NATIVE_DEPS")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.parent().unwrap().join(".runtime/native-deps"));
    println!("cargo:rerun-if-env-changed=KOMA_NATIVE_DEPS");
    println!("cargo:rerun-if-changed=native/jpeg_cmyk.c");
    cc::Build::new()
        .file("native/jpeg_cmyk.c")
        .include(native.join("opencv-4.11.0/3rdparty/libjpeg-turbo/src"))
        .include(native.join("build/3rdparty/libjpeg-turbo"))
        .compile("koma_jpeg_bridge");
    println!(
        "cargo:rustc-link-search=native={}",
        native.join("build/3rdparty/lib").display()
    );
    println!("cargo:rustc-link-lib=static=libjpeg-turbo");
}
