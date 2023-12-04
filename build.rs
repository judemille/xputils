use rustc_version::{version_meta, Channel};

fn main() {
    if matches!(
        version_meta().unwrap().channel,
        Channel::Nightly | Channel::Dev
    ) {
        println!("cargo:rustc-cfg=RUSTC_IS_NIGHTLY");
    }
}
