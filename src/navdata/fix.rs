#![allow(clippy::module_name_repetitions)]
// SPDX-FileCopyrightText: 2024 Julia DeMille <me@jdemille.com>
//
// SPDX-License-Identifier: Parity-7.0.0

//! Structures and parsers for XPFIX1200 and XPFIX1101.
//! Older versions of navdata are not supported.

use std::io::{BufRead, Read};

use itertools::Itertools;
use snafu::ensure;
use winnow::{
    ascii::{dec_uint, float, space0, space1},
    combinator::{opt, preceded},
    prelude::*,
    stream::AsChar,
    trace::trace,
    Located, PResult,
};

use crate::navdata::{
    take_hstring_till, BadLastLineSnafu, DataVersion, Header, ParseError,
    ParseSnafu, UnsupportedVersionSnafu,
};

#[derive(Debug)]
pub(super) struct Fixes {
    pub header: Header,
    pub entries: Vec<Fix>,
}

#[derive(Debug, Clone)]
pub struct Fix {
    pub lat: f64,
    pub lon: f64,
    pub ident: heapless::String<8>,
    /// The airport terminal area this waypoint belongs to, or `ENRT` for enroute waypoints.
    pub terminal_region: heapless::String<4>,
    /// The ICAO region code, according to ICAO document No. 7910.
    pub icao_region: heapless::String<2>,
    /// The type of waypoint this is.
    pub typ: FixType,
    /// The function of this waypoint.
    pub func: FixFunction,
    /// The type of procedure this waypoint belongs to.
    pub proc: FixProcedure,
    /// The printed or spoken name of this waypoint.
    pub printed_spoken_name: Option<heapless::String<32>>,
}

#[derive(Debug, Clone, Copy)]
/// First column of the "Waypoint Type" field.
pub enum FixType {
    /// ARC Center Fix Waypoint
    ArcCenterFix,
    /// Combined Named Intersection and RNAV
    NamedIntxAndRnav,
    /// Unnamed, Charted Intersection
    UnnamedChartedIntx,
    /// Middle Marker as Waypoint
    MiddleMarker,
    /// (Terminal) NDB Navaid as Waypoint
    NdbAsWpt,
    /// Outer Marker as Waypoint
    OuterMarker,
    /// Named Intersection
    NamedIntx,
    /// Uncharted Airway Intersection
    UnchartedAwyIntx,
    /// VFR Waypoint
    VfrWpt,
    /// RNAV Waypoint
    RnavWpt,
    Unspecified,
    Unrecognized(u8),
}

#[derive(Debug, Clone, Copy)]
/// Second column of the "Waypoint Type" field.
pub enum FixFunction {
    /// Final Approach Fix
    FinalAppFix,
    /// Initial and Final Approach Fix
    InitialAndFinalAppFix,
    /// Final Approach Course Fix
    FinalAppCrsFix,
    /// Intermediate Approach Fix
    IntermediateAppFix,
    /// Off-Route Intersection in the FAA National Reference System
    OffRouteIntxFAA,
    /// Off-Route Intersection
    OffRouteIntx,
    /// Initial Approach Fix
    InitialAppFix,
    /// Final Approach Course Fix at Initial Approach Fix
    FinalAppCrsFixAtIAF,
    /// Final Approach Course Fix at Intermediate Approach Fix
    FinalAppCrsFixAtIF,
    /// Missed Approach Fix
    MissedAppFix,
    /// Initial Approach Fix and Missed Approach Fix
    InitialAppFixAndMAF,
    /// Oceanic Entry/Exit Waypoint
    OceanicEntryExitWpt,
    /// Unnamed Stepdown Fix
    UnnamedStepdownFix,
    /// Pitch and Catch Point in the FAA High Altitude Redesign
    PitchAndCatchPoint,
    /// Named Stepdown Fix
    NamedStepdownFix,
    /// AACAA and SUA Waypoints in the FAA High Altitude Redesign
    AacaaAndSuaWpt,
    /// FIR/UIR or Controlled Airspace Intersection
    FirUirCtrlIntx,
    /// Latitude/Longitude Intersection, Full Degree of Latitude
    LatLonFullDegIntx,
    /// Latitude/Longitude Intersection, Half Degree of Latitude
    LatLonHalfDegIntx,
    Unspecified,
    Unrecognized(u8),
}

#[derive(Debug, Clone, Copy)]
/// What procedures this fix is on, if any.
pub enum FixProcedure {
    /// A Standard Instrument Departure.
    SID,
    /// A Standard Terminal Arrival Route.
    STAR,
    /// An approach procedure.
    Approach,
    /// This fix is on multiple procedure types.
    Multiple,
    /// The navdata does not specify.
    Unspecified,
    Unrecognized(u8),
}

pub(super) fn parse_file_buffered<F: Read + BufRead>(
    file: F,
) -> Result<Fixes, ParseError> {
    let mut lines = file.lines();
    let header = super::parse_header(
        |md_type| md_type == "FixXP1100" || md_type == "FixXP1200",
        &mut lines,
    )?;
    ensure!(
        matches!(header.version, DataVersion::XP1101 | DataVersion::XP1200),
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
            let ret = trace("fix row", parse_row)
                .parse(Located::new(&line))
                .map_err(|e| {
                    ParseSnafu {
                        rendered: e.to_string(),
                        stage: "fix row",
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
    Ok(Fixes { header, entries })
}

fn parse_row(input: &mut Located<&str>) -> PResult<Fix> {
    let lat: f64 = trace("latitude", preceded(space0, float)).parse_next(input)?;
    let lon: f64 = trace("longitude", preceded(space1, float)).parse_next(input)?;
    let ident = trace("ident", take_hstring_till::<8, _>(AsChar::is_space))
        .parse_next(input)?;
    let terminal_area =
        trace("terminal area", take_hstring_till::<4, _>(AsChar::is_space))
            .parse_next(input)?;
    let icao_region = trace(
        "ICAO region code",
        take_hstring_till::<2, _>(AsChar::is_space),
    )
    .parse_next(input)?;
    let funny_flags: u32 =
        trace("waypoint flags", preceded(space1, dec_uint)).parse_next(input)?;
    let (typ, func, proc) = parse_wpt_flags(funny_flags, terminal_area != "ENRT");
    let printed_spoken_name =
        opt(preceded(space1, take_hstring_till(|_| false))).parse_next(input)?;
    Ok(Fix {
        lat,
        lon,
        ident,
        terminal_region: terminal_area,
        icao_region,
        typ,
        func,
        proc,
        printed_spoken_name,
    })
}

fn parse_wpt_flags(
    flags: u32,
    terminal: bool,
) -> (FixType, FixFunction, FixProcedure) {
    let bytes = flags.to_le_bytes();
    let typ = match bytes[0] {
        b'A' => FixType::ArcCenterFix,
        b'C' => FixType::NamedIntxAndRnav,
        b'I' => FixType::UnnamedChartedIntx,
        b'M' => FixType::MiddleMarker,
        b'N' => FixType::NdbAsWpt,
        b'O' => FixType::OuterMarker,
        b'R' => FixType::NamedIntx,
        b'V' => FixType::VfrWpt,
        b'W' => FixType::RnavWpt,
        b' ' => FixType::Unspecified,
        _ => FixType::Unrecognized(bytes[0]),
    };
    let func = match bytes[1] {
        b'A' => FixFunction::FinalAppFix,
        b'B' => FixFunction::InitialAndFinalAppFix,
        b'C' => FixFunction::FinalAppCrsFix,
        b'D' => FixFunction::IntermediateAppFix,
        b'E' => FixFunction::OffRouteIntxFAA,
        b'F' => FixFunction::OffRouteIntx,
        b'I' => FixFunction::InitialAppFix,
        b'K' => FixFunction::FinalAppCrsFixAtIAF,
        b'L' => FixFunction::FinalAppCrsFixAtIF,
        b'M' => FixFunction::MissedAppFix,
        b'N' => FixFunction::InitialAppFixAndMAF,
        b'O' => FixFunction::OceanicEntryExitWpt,
        b'P' => {
            if terminal {
                FixFunction::UnnamedStepdownFix
            } else {
                FixFunction::PitchAndCatchPoint
            }
        },
        b'S' => {
            if terminal {
                FixFunction::NamedStepdownFix
            } else {
                FixFunction::AacaaAndSuaWpt
            }
        },
        b'U' => FixFunction::FirUirCtrlIntx,
        b'V' => FixFunction::LatLonFullDegIntx,
        b'W' => FixFunction::LatLonHalfDegIntx,
        b' ' => FixFunction::Unspecified,
        _ => FixFunction::Unrecognized(bytes[1]),
    };
    let proc = match bytes[2] {
        b'D' => FixProcedure::SID,
        b'E' => FixProcedure::STAR,
        b'F' => FixProcedure::Approach,
        b'Z' => FixProcedure::Multiple,
        b' ' => FixProcedure::Unspecified,
        _ => FixProcedure::Unrecognized(bytes[2]),
    };
    (typ, func, proc)
}
