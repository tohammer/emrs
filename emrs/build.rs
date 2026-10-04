fn main() {
    let target = std::env::var("TARGET").expect("cargo sets TARGET");
    println!("cargo:rustc-env=EMRS_TARGET={target}");
}
