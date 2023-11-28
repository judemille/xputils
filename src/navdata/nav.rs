#![allow(clippy::module_name_repetitions)]
//! Structures and parsers for XPNAV1200 and XPNAV1150.
//! Older versions of navdata are not supported.

use rust_decimal::Decimal;
use winnow::{
    ascii::{digit1, line_ending},
    combinator::{cut_err, dispatch, fail, repeat_till0, success, todo, peek},
    error::StrContext,
    prelude::*,
    token::{one_of, take, take_till0},
};

pub struct NavData {
    pub version: XPNavVersion,
    pub cycle: u16,
    pub build: u32,
    pub copyright: String,
    pub navaids: Vec<Navaid>,
}

#[derive(Debug, Copy, Clone)]
pub enum XPNavVersion {
    XPNav1150,
    XPNav1200,
}

/// A navaid.
pub struct Navaid {
    pub lat: Decimal,
    pub lon: Decimal,
    pub elevation: i32,
    pub icao_region_code: heapless::String<2>,
    pub ident: String,
    pub type_data: TypeSpecificData,
}

pub enum TypeSpecificData {
    NDB {
        /// The frequency of this NDB, in whole kHz.
        freq_khz: u16,
        class: NdbClass,
        /// 1.0 if use of BFO is required.
        /// 0.0 otherwise.
        /// Only present in XPNAV1200.
        flags: f32,
        /// The terminal region this NDB belongs to, or `ENRT` for en-route NDBs.
        terminal_region: heapless::String<4>,
        /// The name of this NDB.
        name: String,
    },
    VOR {
        /// The frequency of this VOR, in 10s of kHz.
        freq_10khz: u32,
        class: VorClass,
        slaved_variation: Decimal,
        name: String,
    },
    Localizer {
        is_with_ils: bool,
        freq_10khz: u32,
        max_range: u16,
        crs_mag: f32,
        crs_true: f32,
        airport_icao: heapless::String<4>,
        rwy: heapless::String<3>,
        /// Per documentation, this will be one of:
        /// - `ILS-cat-(I|II|III)`
        /// - `LOC`
        /// - `LDA`
        /// - `SDF`
        name: String,
    },
    Glideslope {
        freq_10khz: u32,
        max_range: u16,
        loc_crs_true: f32,
        glide_angle: u16,
        airport_icao: heapless::String<4>,
        rwy: heapless::String<3>,
        /// Pretty sure this should always be "GS".
        name: String,
    },
    MarkerBeacon {
        typ: MarkerType,
        loc_crs_true: f32,
        airport_icao: heapless::String<4>,
        rwy: heapless::String<3>,
        name: heapless::String<2>,
    },
    DME {
        display_freq: bool,
        paired_freq_10khz: i32,
        service_volume: u16,
        bias: f32,
        airport_icao: heapless::String<4>,
        name: String,
    },
    FPAP {
        channel: u32,
        length_offset: f32,
        final_app_crs_true: f32,
        airport_icao: heapless::String<4>,
        rwy: heapless::String<3>,
        perf: String,
    },
    ThresholdPoint {
        channel: u32,
        thres_cross_height: f32,
        final_app_crs_true: f32,
        glide_path_angle: f32,
        airport_icao: heapless::String<4>,
        rwy: heapless::String<3>,
        /// For the RNAV (GPS) Y 16C at KSEA, this will return
        /// - in XPNAV1200: `W16B`
        /// - in XPNAV1150: `WAAS`
        ///
        /// This is the ref path identifier in XPNAV1200, and the
        /// provider (e.g. WAAS/EGNOS/MSAS/GP) in XPNAV1150.
        /// `GP` means unspecified or GLS.
        ref_path_ident: String,
    },
    GLS {
        channel: u32,
        final_app_crs_true: f32,
        glide_path_angle: f32,
        airport_icao: heapless::String<4>,
        rwy: heapless::String<3>,
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

#[allow(dead_code, unused_variables, clippy::todo)]
fn parse_xpnav(input: &mut &str) -> PResult<Vec<Navaid>> {
    one_of(['A', 'I']).parse_next(input)?; // Byte order marker. Irrelevant.
    line_ending.parse_next(input)?; // Trim off the last line's break.
    let version = dispatch! {take(4usize);
        "1150" => success(XPNavVersion::XPNav1150),
        "1200" => success(XPNavVersion::XPNav1200),
        _ => fail
    }
    .parse_next(input)?;
    " -  data cycle ".parse_next(input)?;
    let cycle: u16 = take(4u8)
        .and_then(digit1)
        .try_map(|s: &str| s.parse())
        .parse_next(input)?;

    ", build ".parse_next(input)?;
    let build: u32 = take(8u8)
        .and_then(digit1)
        .try_map(|s: &str| s.parse())
        .parse_next(input)?;

    ", metadata NavXP".parse_next(input)?;
    take(4u8).and_then(digit1).parse_next(input)?;
    '.'.parse_next(input)?;
    let copyright: String = take_till0(['\r', '\n']).parse_next(input)?.to_string();
    line_ending.parse_next(input)?; // Trim off the last line's break.
    let (navaids, end): (Vec<Navaid>, &str) =
        repeat_till0(cut_err(parse_row).context(StrContext::Label("row")), "99")
            .parse_next(input)?;

    todo!()
}

fn parse_row(input: &mut &str) -> PResult<Navaid> {
    let navaid = dispatch!{peek(take(2usize));
        " 2" => parse_ndb,
        _ => fail
    }.parse_next(input)?;
    line_ending.parse_next(input)?;
    Ok(navaid)
}

fn parse_ndb(input: &mut &str) -> PResult<Navaid> {
    todo(input)
}
