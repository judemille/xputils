// SPDX-FileCopyrightText: 2024 Julia DeMille <me@jdemille.com
//
// SPDX-License-Identifier: Parity-7.0.0

use std::{fs::File, path::Path};

use snafu::{prelude::*, Whatever};
use xputils::navdata::nav::TypeSpecificData;

#[snafu::report]
fn main() -> Result<(), Whatever> {
    let earth_nav_dat = Path::new(file!())
        .parent()
        .unwrap()
        .join("../xp_nav/earth_nav.dat")
        .canonicalize()
        .whatever_context("Could not canonicalize path!")?;
    println!("File path: {}", earth_nav_dat.display());
    let earth_nav_dat =
        File::open(earth_nav_dat).whatever_context("Could not open earth_nav.dat!")?;
    let navaids = xputils::navdata::nav::parse_file(earth_nav_dat)
        .whatever_context("Could not parse earth_nav.dat!")?;

    println!("\n\nMetadata: {:#?}\n\n", navaids.header());
    for vor in navaids
        .entries()
        .iter()
        .filter(|navaid| matches!(navaid.type_data, TypeSpecificData::Vor { .. }))
        .take(5)
    {
        println!("\nNavaid: {vor:#?}");
    }

    for ndb in navaids
        .entries()
        .iter()
        .filter(|navaid| matches!(navaid.type_data, TypeSpecificData::Ndb { .. }))
        .take(5)
    {
        println!("\nNavaid: {ndb:#?}");
    }

    for navaid in navaids
        .entries()
        .iter()
        .filter(|navaid| {
            matches!(navaid.type_data, TypeSpecificData::ThresholdPoint { .. })
        })
        .take(5)
    {
        println!("\nNavaid: {navaid:#?}");
    }

    Ok(())
}
