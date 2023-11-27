#![allow(clippy::module_name_repetitions)]

use rust_decimal::Decimal;
use winnow::prelude::*;

pub struct Navaids {

}

pub struct Navaid {
    lat: Decimal,
    lon: Decimal,
    elevation: i32,
    icao_region_code: [char; 2]
}

pub enum TypeSpecificData {
    NDB {
        freq_khz: i16,
    }
}
