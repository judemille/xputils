#![allow(clippy::module_name_repetitions)]
// SPDX-FileCopyrightText: 2024 Julia DeMille <me@jdemille.com
//
// SPDX-License-Identifier: Parity-7.0.0

//! Structures and parsers for XPNAV1200 and XPNAV1150.
//! Older versions of navdata are not supported.

use std::io::{BufRead, Read};

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
    token::take_till,
    trace::trace,
};

use crate::navdata::{
    take_hstring_till, BadLastLineSnafu, DataVersion, Header, ParseError,
    ParseSnafu, UnsupportedVersionSnafu,
};

pub(super) struct Navaids {
    pub header: Header,
    pub entries: Vec<Navaid>,
}

#[derive(Debug, Clone)]
/// A navaid.
pub struct Navaid {
    pub lat: f64,
    pub lon: f64,
    pub elevation: i32,
    pub icao_region: heapless::String<2>,
    pub ident: heapless::String<5>,
    pub type_data: TypeSpecificData,
}

#[derive(Debug, Clone)]
pub enum TypeSpecificData {
    Ndb {
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
    Vor {
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
    Dme {
        display_freq: bool,
        paired_freq_10khz: u32,
        service_volume: u16,
        bias: f32,
        terminal_region: heapless::String<4>,
        name: String,
    },
    Fpap {
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
    Gls {
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

#[derive(Debug, Clone, Copy)]
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

    ensure!(
        matches!(header.version, DataVersion::XP1150 | DataVersion::XP1200),
        UnsupportedVersionSnafu {
            version: header.version,
        }
    );

    let mut lines = lines
        .filter(|lin| lin.as_ref().map_or(true, |lin| !lin.is_empty()))
        .peekable();

    #[allow(clippy::let_and_return)]
    // Have to let and return to fix a lifetime error.
    let entries: Result<Vec<_>, ParseError> = lines
        .peeking_take_while(|l| l.as_ref().map_or(true, |l| l != "99"))
        .map(|line| {
            let line = line?;
            let ret =
                trace("parse navaid row", parse_row)
                    .parse(&line)
                    .map_err(|e| {
                        ParseSnafu {
                            rendered: e.to_string(),
                            stage: "navaid row",
                        }
                        .build()
                    });
            ret
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

fn parse_row(input: &mut &str) -> PResult<Navaid> {
    let navaid = trace(
        "match row code and then parse type",
        dispatch! {peek(preceded(space0, dec_uint));
            2 => trace("NDB", parse_ndb),
            3 => trace("VOR", parse_vor),
            4 | 5 => trace("localizer", parse_loc),
            6 => trace("glideslope", parse_gs),
            7..=9 => trace("marker beacon", parse_mkr),
            12 | 13 => trace("DME", parse_dme),
            14 => trace("FPAP", parse_fpap),
            15 => trace("GLS", parse_gls),
            16 => trace("landing threshold point", parse_threshold),
            _ => fail
        },
    )
    .parse_next(input)?;
    Ok(navaid)
}

struct RowLead {
    row_code: u8,
    lat: f64,
    lon: f64,
    elevation: i32,
}

fn parse_row_lead(input: &mut &str) -> PResult<RowLead> {
    let row_code: u8 =
        trace("row code", preceded(space0, dec_uint)).parse_next(input)?;
    let lat: f64 = trace("latitude", preceded(space1, float)).parse_next(input)?;
    let lon: f64 = trace("longitude", preceded(space1, float)).parse_next(input)?;
    let elevation: i32 =
        trace("elevation", preceded(space1, dec_int)).parse_next(input)?;
    Ok(RowLead {
        row_code,
        lat,
        lon,
        elevation,
    })
}

fn parse_ndb(input: &mut &str) -> PResult<Navaid> {
    let lead = trace("row lead", parse_row_lead).parse_next(input)?;
    let freq_khz: u16 =
        trace("frequency, kHz", preceded(space1, dec_uint)).parse_next(input)?;
    let class: NdbClass = trace("class", preceded(space1, dec_uint::<_, u8, _>))
        .parse_next(input)?
        .into();
    let flags: f32 = trace("flags", preceded(space1, float)).parse_next(input)?;
    let ident = trace("ident", take_hstring_till::<5, _>(AsChar::is_space))
        .parse_next(input)?;
    let terminal_region = trace(
        "terminal region",
        take_hstring_till::<4, _>(AsChar::is_space),
    )
    .parse_next(input)?;
    let icao_region_code = trace(
        "ICAO region code",
        take_hstring_till::<2, _>(AsChar::is_space),
    )
    .parse_next(input)?;
    let name = trace("name", delimited(space1, rest, space0))
        .parse_next(input)?
        .to_owned();
    Ok(Navaid {
        lat: lead.lat,
        lon: lead.lon,
        elevation: lead.elevation,
        icao_region: icao_region_code,
        ident,
        type_data: TypeSpecificData::Ndb {
            freq_khz,
            class,
            flags,
            terminal_region,
            name,
        },
    })
}

fn parse_vor(input: &mut &str) -> PResult<Navaid> {
    let lead = trace("row lead", parse_row_lead).parse_next(input)?;
    let freq_10khz: u32 =
        trace("frequency, 10 kHz", preceded(space1, dec_uint)).parse_next(input)?;
    let class: VorClass = trace("class", preceded(space1, dec_uint::<_, u8, _>))
        .parse_next(input)?
        .into();
    let slaved_variation: f32 =
        trace("slaved variation, degrees", preceded(space1, float))
            .parse_next(input)?;
    let ident = trace("ident", take_hstring_till::<5, _>(AsChar::is_space))
        .parse_next(input)?;
    let _ = trace("ensure terminal region for VOR is ENRT", " ENRT")
        .parse_next(input)?;
    let icao_region_code = trace(
        "ICAO region code",
        take_hstring_till::<2, _>(AsChar::is_space),
    )
    .parse_next(input)?;
    let name = trace("name", delimited(space1, rest, space0))
        .parse_next(input)?
        .to_owned();
    Ok(Navaid {
        lat: lead.lat,
        lon: lead.lon,
        elevation: lead.elevation,
        icao_region: icao_region_code,
        ident,
        type_data: TypeSpecificData::Vor {
            freq_10khz,
            class,
            slaved_variation,
            name,
        },
    })
}

#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn parse_loc(input: &mut &str) -> PResult<Navaid> {
    let lead = trace("row lead", parse_row_lead).parse_next(input)?;
    let is_with_ils = match lead.row_code {
        4 => true,
        5 => false,
        _ => unreachable!("What the hell?"),
    };
    let freq_10khz: u32 =
        trace("frequency, 10 kHz", preceded(space1, dec_uint)).parse_next(input)?;
    let max_range: u16 =
        trace("maximum reception range", preceded(space1, dec_uint))
            .parse_next(input)?;
    // Listen, the specification about the way this number works is really funny.
    let funny_number: Decimal = trace(
        "funny course true + mag number",
        preceded(space1, take_till(1.., |c: char| c.is_space())),
    )
    .try_map(|s: &str| s.parse())
    .parse_next(input)?;
    let crs_true = funny_number % dec!(360);
    let crs_mag: f32 = ((funny_number - crs_true) / dec!(360))
        .to_f32()
        .unwrap_or(f32::NAN);
    let crs_true: f32 = crs_true.to_f32().unwrap_or(f32::NAN);
    let ident = trace("ident", take_hstring_till::<5, _>(AsChar::is_space))
        .parse_next(input)?;
    let airport_icao = trace(
        "airport ICAO code",
        take_hstring_till::<4, _>(AsChar::is_space),
    )
    .parse_next(input)?;
    let icao_region_code = trace(
        "ICAO region code",
        take_hstring_till::<2, _>(AsChar::is_space),
    )
    .parse_next(input)?;
    let rwy = trace("runway", take_hstring_till::<3, _>(AsChar::is_space))
        .parse_next(input)?;
    let name = trace("name", delimited(space1, rest, space0))
        .parse_next(input)?
        .to_owned();
    Ok(Navaid {
        lat: lead.lat,
        lon: lead.lon,
        elevation: lead.elevation,
        icao_region: icao_region_code,
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
    let lead = trace("row lead", parse_row_lead).parse_next(input)?;
    let freq_10khz: u32 =
        trace("frequency, 10 kHz", preceded(space1, dec_uint)).parse_next(input)?;
    let max_range: u16 =
        trace("maximum reception range", preceded(space1, dec_uint))
            .parse_next(input)?;
    // Listen, the specification about the way this number works is really funny.
    let funny_number: Decimal = trace(
        "funny course true + glide angle number",
        preceded(space1, take_till(1.., |c: char| c.is_space())),
    )
    .try_map(|s: &str| s.parse())
    .parse_next(input)?;
    let loc_crs_true = (funny_number % dec!(1000)).to_f32().unwrap_or(f32::NAN);
    let glide_angle = (funny_number / dec!(1000))
        .trunc()
        .to_u16()
        .unwrap_or(u16::MAX);
    let ident = trace("ident", take_hstring_till::<5, _>(AsChar::is_space))
        .parse_next(input)?;
    let airport_icao = trace(
        "airport ICAO code",
        take_hstring_till::<4, _>(AsChar::is_space),
    )
    .parse_next(input)?;
    let icao_region_code = trace(
        "ICAO region code",
        take_hstring_till::<2, _>(AsChar::is_space),
    )
    .parse_next(input)?;
    let rwy = trace("runway", take_hstring_till::<3, _>(AsChar::is_space))
        .parse_next(input)?;
    let name = trace("name", delimited(space1, rest, space0))
        .parse_next(input)?
        .to_owned();
    Ok(Navaid {
        lat: lead.lat,
        lon: lead.lon,
        elevation: lead.elevation,
        icao_region: icao_region_code,
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
    let lead = trace("row lead", parse_row_lead).parse_next(input)?;
    let typ = match lead.row_code {
        7 => MarkerType::Outer,
        8 => MarkerType::Middle,
        9 => MarkerType::Inner,
        _ => unreachable!("We should not have gotten here."),
    };
    let _ = trace("unused number", preceded(space1, digit1)).parse_next(input)?;
    let _ = trace("unused number", preceded(space1, digit1)).parse_next(input)?;
    let loc_crs_true: f32 =
        trace("localizer course, true degrees", preceded(space1, float))
            .parse_next(input)?;
    let ident = trace("ident", take_hstring_till::<5, _>(AsChar::is_space))
        .parse_next(input)?;
    let airport_icao = trace(
        "airport ICAO code",
        take_hstring_till::<4, _>(AsChar::is_space),
    )
    .parse_next(input)?;
    let icao_region_code = trace(
        "ICAO region code",
        take_hstring_till::<2, _>(AsChar::is_space),
    )
    .parse_next(input)?;
    let rwy = trace("runway", take_hstring_till::<3, _>(AsChar::is_space))
        .parse_next(input)?;
    let name = trace("name", take_hstring_till::<2, _>(AsChar::is_space))
        .parse_next(input)?;
    Ok(Navaid {
        lat: lead.lat,
        lon: lead.lon,
        elevation: lead.elevation,
        icao_region: icao_region_code,
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
    let lead = trace("row lead", parse_row_lead).parse_next(input)?;
    let display_freq = match lead.row_code {
        12 => false,
        13 => true,
        _ => unreachable!(),
    };
    let paired_freq_10khz: u32 =
        trace("paired frequency, 10 kHz", preceded(space1, dec_uint))
            .parse_next(input)?;
    let service_volume: u16 =
        trace("service volume", preceded(space1, dec_uint)).parse_next(input)?;
    let bias: f32 = trace("bias", preceded(space1, float)).parse_next(input)?;
    let ident = trace("ident", take_hstring_till::<5, _>(AsChar::is_space))
        .parse_next(input)?;
    let terminal_region = trace(
        "terminal region",
        take_hstring_till::<4, _>(AsChar::is_space),
    )
    .parse_next(input)?;
    let icao_region_code = trace(
        "ICAO region code",
        take_hstring_till::<2, _>(AsChar::is_space),
    )
    .parse_next(input)?;
    let name = trace("name", delimited(space1, rest, space0))
        .parse_next(input)?
        .to_owned();
    Ok(Navaid {
        lat: lead.lat,
        lon: lead.lon,
        elevation: lead.elevation,
        icao_region: icao_region_code,
        ident,
        type_data: TypeSpecificData::Dme {
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
    let lead = trace("row lead", parse_row_lead).parse_next(input)?;
    let channel: u32 =
        trace("channel", preceded(space1, dec_uint)).parse_next(input)?;
    let length_offset: f32 =
        trace("length offset", preceded(space1, float)).parse_next(input)?;
    let final_app_crs_true: f32 = trace(
        "final approach course, true degrees",
        preceded(space1, float),
    )
    .parse_next(input)?;
    let ident = trace("ident", take_hstring_till::<5, _>(AsChar::is_space))
        .parse_next(input)?;
    let airport_icao = trace(
        "airport ICAO code",
        take_hstring_till::<4, _>(AsChar::is_space),
    )
    .parse_next(input)?;
    let icao_region_code = trace(
        "ICAO region code",
        take_hstring_till::<2, _>(AsChar::is_space),
    )
    .parse_next(input)?;
    let rwy = trace("runway", take_hstring_till::<3, _>(AsChar::is_space))
        .parse_next(input)?;
    let perf = trace("performance type", delimited(space1, rest, space0))
        .parse_next(input)?
        .to_owned();
    Ok(Navaid {
        lat: lead.lat,
        lon: lead.lon,
        elevation: lead.elevation,
        icao_region: icao_region_code,
        ident,
        type_data: TypeSpecificData::Fpap {
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
    let lead = trace("row lead", parse_row_lead).parse_next(input)?;
    let channel: u32 =
        trace("channel", preceded(space1, dec_uint)).parse_next(input)?;
    let _ = trace("unused number", preceded(space1, digit1)).parse_next(input)?;
    // Listen, the specification about the way this number works is really funny.
    let funny_number: Decimal = trace(
        "funny final approach course + glide path angle number",
        preceded(space1, take_till(1.., |c: char| c.is_space())),
    )
    .try_map(|s: &str| s.parse())
    .parse_next(input)?;
    let final_app_crs_true =
        (funny_number % dec!(1000)).to_f32().unwrap_or(f32::NAN);
    let glide_path_angle = (funny_number / dec!(1000))
        .trunc()
        .to_u16()
        .unwrap_or(u16::MAX);
    let ident = trace("ident", take_hstring_till::<5, _>(AsChar::is_space))
        .parse_next(input)?;
    let airport_icao = trace(
        "airport ICAO code",
        take_hstring_till::<4, _>(AsChar::is_space),
    )
    .parse_next(input)?;
    let icao_region_code = trace(
        "ICAO region code",
        take_hstring_till::<2, _>(AsChar::is_space),
    )
    .parse_next(input)?;
    let rwy = trace("runway", take_hstring_till::<3, _>(AsChar::is_space))
        .parse_next(input)?;
    let ref_path_ident = trace("ref path ident", delimited(space1, rest, space0))
        .parse_next(input)?
        .to_owned();
    Ok(Navaid {
        lat: lead.lat,
        lon: lead.lon,
        elevation: lead.elevation,
        icao_region: icao_region_code,
        ident,
        type_data: TypeSpecificData::Gls {
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
    let lead = trace("row lead", parse_row_lead).parse_next(input)?;
    let channel: u32 =
        trace("channel", preceded(space1, dec_uint)).parse_next(input)?;
    let thres_cross_height: f32 =
        trace("threshold crossing height", preceded(space1, float))
            .parse_next(input)?;
    // Listen, the specification about the way this number works is really funny.
    let funny_number: Decimal = trace(
        "funny final approach course + glide path angle number",
        preceded(space1, take_till(1.., |c: char| c.is_space())),
    )
    .try_map(|s: &str| s.parse())
    .parse_next(input)?;
    let final_app_crs_true =
        (funny_number % dec!(1000)).to_f32().unwrap_or(f32::NAN);
    let glide_path_angle = (funny_number / dec!(1000))
        .trunc()
        .to_u16()
        .unwrap_or(u16::MAX);
    let ident = trace("ident", take_hstring_till::<5, _>(AsChar::is_space))
        .parse_next(input)?;
    let airport_icao = trace(
        "airport ICAO code",
        take_hstring_till::<4, _>(AsChar::is_space),
    )
    .parse_next(input)?;
    let icao_region_code = trace(
        "ICAO region code",
        take_hstring_till::<2, _>(AsChar::is_space),
    )
    .parse_next(input)?;
    let rwy = trace("runway", take_hstring_till::<3, _>(AsChar::is_space))
        .parse_next(input)?;
    let ref_path_ident = trace("ref path ident", delimited(space1, rest, space0))
        .parse_next(input)?
        .to_owned();
    Ok(Navaid {
        lat: lead.lat,
        lon: lead.lon,
        elevation: lead.elevation,
        icao_region: icao_region_code,
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
