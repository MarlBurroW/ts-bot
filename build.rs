fn main() {
    // Link to Porcupine shared library
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let porcupine_dir = std::path::Path::new(&manifest_dir).join("porcupine");
    println!("cargo:rustc-link-search=native={}", porcupine_dir.display());
    println!("cargo:rustc-link-lib=dylib=pv_porcupine");
    // Set rpath so the binary finds the .so at runtime
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", porcupine_dir.display());
}
