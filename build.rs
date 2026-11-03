// Links assets/lumen.res, the exe icon (rebuild it with `python assets/make_res.py`).
fn main() {
    let res = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("assets").join("lumen.res");
    println!("cargo:rerun-if-changed={}", res.display());
    println!("cargo:rustc-link-arg-bins={}", res.display());
}
