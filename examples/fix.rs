// SPDX-FileCopyrightText: 2024 Julia DeMille <me@jdemille.com
//
// SPDX-License-Identifier: Parity-7.0.0

use std::{fs::File, path::Path};

use snafu::{prelude::*, Whatever};
use xputils::navdata::fix::{FixFunction, FixType};

#[snafu::report]
fn main() -> Result<(), Whatever> {
    let earth_fix_dat = Path::new(file!())
        .parent()
        .unwrap()
        .join("../xp_nav/earth_fix.dat")
        .canonicalize()
        .whatever_context("Could not canonicalize path!")?;
    println!("File path: {}", earth_fix_dat.display());
    let earth_fix_dat =
        File::open(earth_fix_dat).whatever_context("Could not open earth_fix.dat!")?;
    let fixes = xputils::navdata::fix::parse_file(earth_fix_dat)
        .whatever_context("Could not parse earth_nav.dat!")?;

    println!("\n\nMetadata: {:#?}\n\n", fixes.header());
    for fix in fixes
        .entries()
        .iter()
        .filter(|fix| {
            !matches!(fix.func, FixFunction::Unspecified)
                && !matches!(fix.typ, FixType::Unspecified)
        })
        .take(20)
    {
        println!("\nFix: {fix:#?}");
    }

    Ok(())
}
