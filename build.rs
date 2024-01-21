// SPDX-FileCopyrightText: 2024 Julia DeMille <me@jdemille.com>
//
// SPDX-License-Identifier: Parity-7.0.0

use rustc_version::{version_meta, Channel};

fn main() {
    if matches!(
        version_meta().unwrap().channel,
        Channel::Nightly | Channel::Dev
    ) {
        println!("cargo:rustc-cfg=RUSTC_IS_NIGHTLY");
    }
}
