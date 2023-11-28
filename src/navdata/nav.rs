#![allow(clippy::module_name_repetitions)]

use rust_decimal::Decimal;
use winnow::prelude::*;

pub struct Navaid {
    pub lat: Decimal,
    pub lon: Decimal,
    pub elevation: i32,
    pub icao_region_code: [u8; 2],
    pub ident: String,
    pub type_data: TypeSpecificData,
}

pub enum TypeSpecificData {
    NDB {
        freq_khz: i16,
        class: NdbClass,
        flags: f32,
        terminal_region: [u8; 4],
        name: String,
    },
    VOR {
        freq_10khz: i32,
        class: VorClass,
        slaved_variation: Decimal,
        name: String,
    },
    Localizer {
        is_with_ils: bool,
        freq_10khz: i32,
        max_range: i32,
        crs_mag: f32,
        crs_true: f32,
        airport_icao: [u8; 4],
        rwy: [u8; 3],
        /// Per documentation, this will be one of:
        /// - `ILS-cat-(I|II|III)`
        /// - `LOC`
        /// - `LDA`
        /// - `SDF`
        name: String,
    },
    Glideslope {
        freq_10khz: i32,
        max_range: i32,
        loc_crs_true: f32,
        glide_angle: u16,
        airport_icao: [u8; 4],
        rwy: [u8; 3],
        /// Pretty sure this should always be "GS".
        name: String,
    },
    MarkerBeacon {
        typ: MarkerType,
        loc_crs_true: f32,
        airport_icao: [u8; 4],
        rwy: [u8; 3],
        name: [u8; 2],
    },
    DME {
        display_freq: bool,
        paired_freq_10khz: i32,
        service_volume: u16,
        bias: f32,
        airport_icao: [u8; 4],
        name: String,
    },
    FPAP {
        channel: u32,
        length_offset: f32,
        final_app_crs_true: f32,
        airport_icao: [u8; 4],
        rwy: [u8; 3],
        perf: String,
    },
    ThresholdPoint {
        channel: u32,
        thres_cross_height: f32,
        final_app_crs_true: f32,
        glide_path_angle: f32,
        airport_icao: [u8; 4],
        rwy: [u8; 3],
        ref_path_ident: String,
    },
    GLS {
        channel: u32,
        final_app_crs_true: f32,
        glide_path_angle: f32,
        airport_icao: [u8; 4],
        rwy: [u8; 3],
        /// I think this should be `GLS`.
        ref_path_ident: String,
    },
}

pub enum NdbClass {
    Locator = 15,
    LowPower = 25,
    Normal = 50,
    HighPower = 75,
}

pub enum VorClass {
    /// Terminal, low power.
    Terminal = 25,
    /// Low altitude, medium power.
    LowAlt = 40,
    /// High altitude, high power.
    HighAlt = 130,
    /// Unspecified, but likely high power.
    Unspecified = 125,
}

pub enum MarkerType {
    Outer,
    Middle,
    Inner,
}
