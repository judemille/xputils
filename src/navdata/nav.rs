#![allow(clippy::module_name_repetitions)]
//! Structures and parsers for XPNAV1200 and XPNAV1150.
//! Older versions of navdata are not supported.

use std::{
    io::{BufRead, Read},
    sync::Arc,
};

use itertools::Itertools;
use num_enum::{FromPrimitive, IntoPrimitive};
use rust_decimal::prelude::*;
use rust_decimal_macros::dec;
use snafu::ensure;
use winnow::{
    ascii::{dec_int, dec_uint, digit1, float, space0, space1},
    combinator::{delimited, dispatch, fail, peek, preceded, rest},
    prelude::*,
    stream::AsChar,
    token::take_till1,
};

use crate::navdata::{
    parse_fixed_str, BadLastLineSnafu, DataVersion, Header, ParseError, ParseSnafu,
    UnsupportedVersionSnafu,
};

pub struct Navaids {
    header: Header,
    pub(super) entries: Vec<Arc<Navaid>>,
}

impl Navaids {
    #[must_use]
    pub fn header(&self) -> &Header {
        &self.header
    }

    #[must_use]
    pub fn entries(&self) -> &Vec<Arc<Navaid>> {
        &self.entries
    }

    pub fn entries_mut(&mut self) -> &mut Vec<Arc<Navaid>> {
        &mut self.entries
    }
}

#[derive(Debug)]
/// A navaid.
pub struct Navaid {
    pub lat: f64,
    pub lon: f64,
    pub elevation: i32,
    pub icao_region_code: heapless::String<2>,
    pub ident: heapless::String<5>,
    pub type_data: TypeSpecificData,
}

#[derive(Debug)]
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
        slaved_variation: f32,
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
        /// Hundredths of a degree. `u16::MAX` should be interpreted as an error.
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
        paired_freq_10khz: u32,
        service_volume: u16,
        bias: f32,
        terminal_region: heapless::String<4>,
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
        /// Hundredths of a degree. `u16::MAX` should be interpreted as an error.
        glide_path_angle: u16,
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
        /// Hundredths of a degree. `u16::MAX` should be interpreted as an error.
        glide_path_angle: u16,
        airport_icao: heapless::String<4>,
        rwy: heapless::String<3>,
        /// I think this should be `GLS`.
        ref_path_ident: String,
    },
}

#[repr(u8)]
#[derive(Debug, Copy, Clone, FromPrimitive, IntoPrimitive)]
pub enum NdbClass {
    Locator = 15,
    LowPower = 25,
    Normal = 50,
    HighPower = 75,
    #[num_enum(catch_all)]
    Unrecognized(u8),
}

#[repr(u8)]
#[derive(Debug, Copy, Clone, FromPrimitive, IntoPrimitive)]
pub enum VorClass {
    /// Terminal, low power.
    Terminal = 25,
    /// Low altitude, medium power.
    LowAlt = 40,
    /// High altitude, high power.
    HighAlt = 130,
    /// Unspecified, but likely high power.
    Unspecified = 125,
    #[num_enum(catch_all)]
    /// xputils does not recognize this value.
    Unrecognized(u8),
}

#[derive(Debug)]
pub enum MarkerType {
    Outer,
    Middle,
    Inner,
}

pub(super) fn parse_file_buffered<F: Read + BufRead>(
    file: F,
) -> Result<Navaids, ParseError> {
    let mut lines = file.lines();
    let header = super::parse_header(
        |md_type| md_type == "NavXP1200" || md_type == "NavXP1150",
        &mut lines,
    )?;
    if !matches!(header.version, DataVersion::XP1150 | DataVersion::XP1200) {
        return UnsupportedVersionSnafu {
            version: header.version,
        }
        .fail();
    }
    let mut lines = lines
        .filter(|lin| lin.as_ref().map_or(true, |lin| !lin.is_empty()))
        .peekable();
    let entries: Result<Vec<_>, ParseError> = lines
        .peeking_take_while(|l| l.as_ref().map_or(true, |l| l != "99"))
        .map(|line| {
            parse_row.parse(&line?).map_err(|e| {
                ParseSnafu {
                    rendered: e.to_string(),
                    stage: "navaid row",
                }
                .build()
            })
        })
        .collect();
    let entries = entries?;
    lines
        .next()
        .ok_or_else(|| ParseError::MissingLine)
        .and_then(|last_line| {
            let last_line = last_line?;
            ensure!(last_line == "99", BadLastLineSnafu { last_line });
            Ok(())
        })?;
    Ok(Navaids { header, entries })
}

fn parse_row(input: &mut &str) -> PResult<Arc<Navaid>> {
    let navaid = dispatch! {peek(preceded(space0, dec_uint));
        2 => parse_ndb,
        3 => parse_vor,
        4 | 5 => parse_loc,
        6 => parse_gs,
        7..=9 => parse_mkr,
        12 | 13 => parse_dme,
        14 => parse_fpap,
        15 => parse_gls,
        16 => parse_threshold,
        _ => fail
    }
    .parse_next(input)?;
    Ok(Arc::new(navaid))
}

struct RowLead {
    row_code: u8,
    lat: f64,
    lon: f64,
    elevation: i32,
}

fn parse_row_lead(input: &mut &str) -> PResult<RowLead> {
    let row_code: u8 = preceded(space0, dec_uint).parse_next(input)?;
    let lat: f64 = preceded(space1, float).parse_next(input)?;
    let lon: f64 = preceded(space1, float).parse_next(input)?;
    let elevation: i32 = preceded(space1, dec_int).parse_next(input)?;
    Ok(RowLead {
        row_code,
        lat,
        lon,
        elevation,
    })
}

fn parse_ndb(input: &mut &str) -> PResult<Navaid> {
    let lead = parse_row_lead.parse_next(input)?;
    let freq_khz: u16 = preceded(space1, dec_uint).parse_next(input)?;
    let class: NdbClass = preceded(space1, dec_uint::<_, u8, _>)
        .parse_next(input)?
        .into();
    let flags: f32 = preceded(space1, float).parse_next(input)?;
    let ident = parse_fixed_str::<5>.parse_next(input)?;
    let terminal_region = parse_fixed_str::<4>.parse_next(input)?;
    let icao_region_code = parse_fixed_str::<2>.parse_next(input)?;
    let name = delimited(space1, rest, space0)
        .parse_next(input)?
        .to_owned();
    Ok(Navaid {
        lat: lead.lat,
        lon: lead.lon,
        elevation: lead.elevation,
        icao_region_code,
        ident,
        type_data: TypeSpecificData::NDB {
            freq_khz,
            class,
            flags,
            terminal_region,
            name,
        },
    })
}

fn parse_vor(input: &mut &str) -> PResult<Navaid> {
    let lead = parse_row_lead.parse_next(input)?;
    let freq_10khz: u32 = preceded(space1, dec_uint).parse_next(input)?;
    let class: VorClass = preceded(space1, dec_uint::<_, u8, _>)
        .parse_next(input)?
        .into();
    let slaved_variation: f32 = preceded(space1, float).parse_next(input)?;
    let ident = parse_fixed_str::<5>.parse_next(input)?;
    let _ = parse_fixed_str::<4>.parse_next(input)?;
    let icao_region_code = parse_fixed_str::<2>.parse_next(input)?;
    let name = delimited(space1, rest, space0)
        .parse_next(input)?
        .to_owned();
    Ok(Navaid {
        lat: lead.lat,
        lon: lead.lon,
        elevation: lead.elevation,
        icao_region_code,
        ident,
        type_data: TypeSpecificData::VOR {
            freq_10khz,
            class,
            slaved_variation,
            name,
        },
    })
}

#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn parse_loc(input: &mut &str) -> PResult<Navaid> {
    let lead = parse_row_lead.parse_next(input)?;
    let is_with_ils = match lead.row_code {
        4 => true,
        5 => false,
        _ => unreachable!("What the hell?"),
    };
    let freq_10khz: u32 = preceded(space1, dec_uint).parse_next(input)?;
    let max_range: u16 = preceded(space1, dec_uint).parse_next(input)?;
    // Listen, the specification about the way this number works is really funny.
    let funny_number: Decimal = preceded(space1, take_till1(|c: char| c.is_space()))
        .try_map(|s: &str| s.parse())
        .parse_next(input)?;
    let crs_true = funny_number % dec!(360);
    let crs_mag: f32 = ((funny_number - crs_true) / dec!(360))
        .to_f32()
        .unwrap_or(f32::NAN);
    let crs_true: f32 = crs_true.to_f32().unwrap_or(f32::NAN);
    let ident = parse_fixed_str::<5>.parse_next(input)?;
    let airport_icao = parse_fixed_str::<4>.parse_next(input)?;
    let icao_region_code = parse_fixed_str::<2>.parse_next(input)?;
    let rwy = parse_fixed_str::<3>.parse_next(input)?;
    let name = delimited(space1, rest, space0)
        .parse_next(input)?
        .to_owned();
    Ok(Navaid {
        lat: lead.lat,
        lon: lead.lon,
        elevation: lead.elevation,
        icao_region_code,
        ident,
        type_data: TypeSpecificData::Localizer {
            is_with_ils,
            freq_10khz,
            max_range,
            crs_mag,
            crs_true,
            airport_icao,
            rwy,
            name,
        },
    })
}

fn parse_gs(input: &mut &str) -> PResult<Navaid> {
    let lead = parse_row_lead.parse_next(input)?;
    let freq_10khz: u32 = preceded(space1, dec_uint).parse_next(input)?;
    let max_range: u16 = preceded(space1, dec_uint).parse_next(input)?;
    // Listen, the specification about the way this number works is really funny.
    let funny_number: Decimal = preceded(space1, take_till1(|c: char| c.is_space()))
        .try_map(|s: &str| s.parse())
        .parse_next(input)?;
    let loc_crs_true = (funny_number % dec!(1000)).to_f32().unwrap_or(f32::NAN);
    let glide_angle = (funny_number / dec!(1000))
        .trunc()
        .to_u16()
        .unwrap_or(u16::MAX);
    let ident = parse_fixed_str::<5>.parse_next(input)?;
    let airport_icao = parse_fixed_str::<4>.parse_next(input)?;
    let icao_region_code = parse_fixed_str::<2>.parse_next(input)?;
    let rwy = parse_fixed_str::<3>.parse_next(input)?;
    let name = delimited(space1, rest, space0)
        .parse_next(input)?
        .to_owned();
    Ok(Navaid {
        lat: lead.lat,
        lon: lead.lon,
        elevation: lead.elevation,
        icao_region_code,
        ident,
        type_data: TypeSpecificData::Glideslope {
            freq_10khz,
            max_range,
            loc_crs_true,
            glide_angle,
            airport_icao,
            rwy,
            name,
        },
    })
}

fn parse_mkr(input: &mut &str) -> PResult<Navaid> {
    let lead = parse_row_lead.parse_next(input)?;
    let typ = match lead.row_code {
        7 => MarkerType::Outer,
        8 => MarkerType::Middle,
        9 => MarkerType::Inner,
        _ => unreachable!("We should not have gotten here."),
    };
    let _ = preceded(space1, digit1).parse_next(input)?;
    let _ = preceded(space1, digit1).parse_next(input)?;
    let loc_crs_true: f32 = preceded(space1, float).parse_next(input)?;
    let ident = parse_fixed_str::<5>.parse_next(input)?;
    let airport_icao = parse_fixed_str::<4>.parse_next(input)?;
    let icao_region_code = parse_fixed_str::<2>.parse_next(input)?;
    let rwy = parse_fixed_str::<3>.parse_next(input)?;
    let name = parse_fixed_str::<2>.parse_next(input)?;
    Ok(Navaid {
        lat: lead.lat,
        lon: lead.lon,
        elevation: lead.elevation,
        icao_region_code,
        ident,
        type_data: TypeSpecificData::MarkerBeacon {
            typ,
            loc_crs_true,
            airport_icao,
            rwy,
            name,
        },
    })
}

fn parse_dme(input: &mut &str) -> PResult<Navaid> {
    let lead = parse_row_lead.parse_next(input)?;
    let display_freq = match lead.row_code {
        12 => false,
        13 => true,
        _ => unreachable!(),
    };
    let paired_freq_10khz: u32 = preceded(space1, dec_uint).parse_next(input)?;
    let service_volume: u16 = preceded(space1, dec_uint).parse_next(input)?;
    let bias: f32 = preceded(space1, float).parse_next(input)?;
    let ident = parse_fixed_str::<5>.parse_next(input)?;
    let terminal_region = parse_fixed_str::<4>.parse_next(input)?;
    let icao_region_code = parse_fixed_str::<2>.parse_next(input)?;
    let name = delimited(space1, rest, space0)
        .parse_next(input)?
        .to_owned();
    Ok(Navaid {
        lat: lead.lat,
        lon: lead.lon,
        elevation: lead.elevation,
        icao_region_code,
        ident,
        type_data: TypeSpecificData::DME {
            display_freq,
            paired_freq_10khz,
            service_volume,
            bias,
            terminal_region,
            name,
        },
    })
}

fn parse_fpap(input: &mut &str) -> PResult<Navaid> {
    let lead = parse_row_lead.parse_next(input)?;
    let channel: u32 = preceded(space1, dec_uint).parse_next(input)?;
    let length_offset: f32 = preceded(space1, float).parse_next(input)?;
    let final_app_crs_true: f32 = preceded(space1, float).parse_next(input)?;
    let ident = parse_fixed_str::<5>.parse_next(input)?;
    let airport_icao = parse_fixed_str::<4>.parse_next(input)?;
    let icao_region_code = parse_fixed_str::<2>.parse_next(input)?;
    let rwy = parse_fixed_str::<3>.parse_next(input)?;
    let perf = delimited(space1, rest, space0)
        .parse_next(input)?
        .to_owned();
    Ok(Navaid {
        lat: lead.lat,
        lon: lead.lon,
        elevation: lead.elevation,
        icao_region_code,
        ident,
        type_data: TypeSpecificData::FPAP {
            channel,
            length_offset,
            final_app_crs_true,
            airport_icao,
            rwy,
            perf,
        },
    })
}

fn parse_gls(input: &mut &str) -> PResult<Navaid> {
    let lead = parse_row_lead.parse_next(input)?;
    let channel: u32 = preceded(space1, dec_uint).parse_next(input)?;
    let _ = preceded(space1, digit1).parse_next(input)?;
    // Listen, the specification about the way this number works is really funny.
    let funny_number: Decimal = preceded(space1, take_till1(|c: char| c.is_space()))
        .try_map(|s: &str| s.parse())
        .parse_next(input)?;
    let final_app_crs_true = (funny_number % dec!(1000)).to_f32().unwrap_or(f32::NAN);
    let glide_path_angle = (funny_number / dec!(1000))
        .trunc()
        .to_u16()
        .unwrap_or(u16::MAX);
    let ident = parse_fixed_str::<5>.parse_next(input)?;
    let airport_icao = parse_fixed_str::<4>.parse_next(input)?;
    let icao_region_code = parse_fixed_str::<2>.parse_next(input)?;
    let rwy = parse_fixed_str::<3>.parse_next(input)?;
    let ref_path_ident = delimited(space1, rest, space0)
        .parse_next(input)?
        .to_owned();
    Ok(Navaid {
        lat: lead.lat,
        lon: lead.lon,
        elevation: lead.elevation,
        icao_region_code,
        ident,
        type_data: TypeSpecificData::GLS {
            channel,
            final_app_crs_true,
            glide_path_angle,
            airport_icao,
            rwy,
            ref_path_ident,
        },
    })
}

fn parse_threshold(input: &mut &str) -> PResult<Navaid> {
    let lead = parse_row_lead.parse_next(input)?;
    let channel: u32 = preceded(space1, dec_uint).parse_next(input)?;
    let thres_cross_height: f32 = preceded(space1, float).parse_next(input)?;
    // Listen, the specification about the way this number works is really funny.
    let funny_number: Decimal = preceded(space1, take_till1(|c: char| c.is_space()))
        .try_map(|s: &str| s.parse())
        .parse_next(input)?;
    let final_app_crs_true = (funny_number % dec!(1000)).to_f32().unwrap_or(f32::NAN);
    let glide_path_angle = (funny_number / dec!(1000))
        .trunc()
        .to_u16()
        .unwrap_or(u16::MAX);
    let ident = parse_fixed_str::<5>.parse_next(input)?;
    let airport_icao = parse_fixed_str::<4>.parse_next(input)?;
    let icao_region_code = parse_fixed_str::<2>.parse_next(input)?;
    let rwy = parse_fixed_str::<3>.parse_next(input)?;
    let ref_path_ident = delimited(space1, rest, space0)
        .parse_next(input)?
        .to_owned();
    Ok(Navaid {
        lat: lead.lat,
        lon: lead.lon,
        elevation: lead.elevation,
        icao_region_code,
        ident,
        type_data: TypeSpecificData::ThresholdPoint {
            channel,
            thres_cross_height,
            final_app_crs_true,
            glide_path_angle,
            airport_icao,
            rwy,
            ref_path_ident,
        },
    })
}
