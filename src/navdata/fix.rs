#![allow(clippy::module_name_repetitions)]
//! Structures and parsers for XPFIX1200 and XPFIX1101.
//! Older versions of navdata are not supported.

use std::{io::{BufRead, BufReader, Read}, sync::Arc};

use itertools::Itertools;
use winnow::{
    ascii::{dec_uint, float, space0, space1},
    combinator::{preceded, rest},
    prelude::*,
    PResult,
};

use crate::navdata::{parse_fixed_str, Header, ParseError, ParseSnafu, StringTooLarge};

#[derive(Debug)]
pub struct Fixes {
    pub header: Header,
    pub entries: Vec<Arc<Waypoint>>,
}

#[derive(Debug)]
pub struct Waypoint {
    pub lat: f64,
    pub lon: f64,
    pub ident: heapless::String<8>,
    /// The airport terminal area this waypoint belongs to, or `ENRT` for enroute waypoints.
    pub terminal_area: heapless::String<4>,
    /// The ICAO region code, according to ICAO document No. 7910.
    pub icao_region: heapless::String<2>,
    /// The type of waypoint this is.
    pub typ: WptType,
    /// The function of this waypoint.
    pub func: WptFunction,
    /// The type of procedure this waypoint belongs to.
    pub proc: WptProcedure,
    /// The printed or spoken name of this waypoint.
    pub printed_spoken_name: heapless::String<32>,
}

#[derive(Debug)]
/// First column of the "Waypoint Type" field.
pub enum WptType {
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

#[derive(Debug)]
/// Second column of the "Waypoint Type" field.
pub enum WptFunction {
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

#[derive(Debug)]
/// What procedures this fix is on, if any.
pub enum WptProcedure {
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

/// Parse a file with the provided [`Read`].
/// If your file handle is already [`BufRead`], you should instead call [`parse_file_buffered`].
///
/// This is suitable for `earth_fix.dat` and `user_fix.dat`.
/// # Errors
/// An error is returned if there is an I/O error, or if the file is malformed.
pub fn parse_file<F: Read>(file: F) -> Result<Fixes, ParseError> {
    parse_file_buffered(BufReader::new(file))
}

/// Parse a file with the provided [`BufRead`].
/// This is suitable for `earth_fix.dat` and `user_fix.dat`.
/// # Errors
/// An error is returned if there is an I/O error, or if the file is malformed.
pub fn parse_file_buffered<F: Read + BufRead>(file: F) -> Result<Fixes, ParseError> {
    let mut lines = file.lines();
    let header = super::parse_header(
        |md_type| md_type == "FixXP1100" || md_type == "FixXP1200",
        &mut lines,
    )?;

    let mut lines = lines
        .filter(|lin| lin.as_ref().map_or(true, |lin| !lin.is_empty()))
        .peekable();
    let entries: Result<Vec<_>, ParseError> = lines
        .peeking_take_while(|l| l.as_ref().map_or(true, |l| l != "99"))
        .map(|line| {
            parse_row.parse(&line?).map_err(|e| {
                ParseSnafu {
                    rendered: e.to_string(),
                    stage: "row",
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
            if last_line == "99" {
                Ok(())
            } else {
                Err(ParseError::BadLastLine { last_line })
            }
        })?;
    Ok(Fixes { header, entries })
}

fn parse_row(input: &mut &str) -> PResult<Arc<Waypoint>> {
    let lat: f64 = preceded(space0, float).parse_next(input)?;
    let lon: f64 = preceded(space1, float).parse_next(input)?;
    let ident = parse_fixed_str::<8>.parse_next(input)?;
    let terminal_area = parse_fixed_str::<4>.parse_next(input)?;
    let icao_region = parse_fixed_str::<2>.parse_next(input)?;
    let funny_flags: u32 = preceded(space1, dec_uint).parse_next(input)?;
    let (typ, func, proc) = parse_wpt_flags(funny_flags, terminal_area != "ENRT");
    let printed_spoken_name = preceded(space1, rest)
        .try_map(|id: &str| {
            heapless::String::<32>::try_from(id).map_err(|()| StringTooLarge)
        })
        .parse_next(input)?;
    Ok(Arc::new(Waypoint {
        lat,
        lon,
        ident,
        terminal_area,
        icao_region,
        typ,
        func,
        proc,
        printed_spoken_name,
    }))
}

fn parse_wpt_flags(flags: u32, terminal: bool) -> (WptType, WptFunction, WptProcedure) {
    let bytes = flags.to_le_bytes();
    let typ = match bytes[0] {
        b'A' => WptType::ArcCenterFix,
        b'C' => WptType::NamedIntxAndRnav,
        b'I' => WptType::UnnamedChartedIntx,
        b'M' => WptType::MiddleMarker,
        b'N' => WptType::NdbAsWpt,
        b'O' => WptType::OuterMarker,
        b'R' => WptType::NamedIntx,
        b'V' => WptType::VfrWpt,
        b'W' => WptType::RnavWpt,
        b' ' => WptType::Unspecified,
        _ => WptType::Unrecognized(bytes[0]),
    };
    let func = match bytes[1] {
        b'A' => WptFunction::FinalAppFix,
        b'B' => WptFunction::InitialAndFinalAppFix,
        b'C' => WptFunction::FinalAppCrsFix,
        b'D' => WptFunction::IntermediateAppFix,
        b'E' => WptFunction::OffRouteIntxFAA,
        b'F' => WptFunction::OffRouteIntx,
        b'I' => WptFunction::InitialAppFix,
        b'K' => WptFunction::FinalAppCrsFixAtIAF,
        b'L' => WptFunction::FinalAppCrsFixAtIF,
        b'M' => WptFunction::MissedAppFix,
        b'N' => WptFunction::InitialAppFixAndMAF,
        b'O' => WptFunction::OceanicEntryExitWpt,
        b'P' => {
            if terminal {
                WptFunction::UnnamedStepdownFix
            } else {
                WptFunction::PitchAndCatchPoint
            }
        },
        b'S' => {
            if terminal {
                WptFunction::NamedStepdownFix
            } else {
                WptFunction::AacaaAndSuaWpt
            }
        },
        b'U' => WptFunction::FirUirCtrlIntx,
        b'V' => WptFunction::LatLonFullDegIntx,
        b'W' => WptFunction::LatLonHalfDegIntx,
        b' ' => WptFunction::Unspecified,
        _ => WptFunction::Unrecognized(bytes[1]),
    };
    let proc = match bytes[2] {
        b'D' => WptProcedure::SID,
        b'E' => WptProcedure::STAR,
        b'F' => WptProcedure::Approach,
        b'Z' => WptProcedure::Multiple,
        b' ' => WptProcedure::Unspecified,
        _ => WptProcedure::Unrecognized(bytes[2]),
    };
    (typ, func, proc)
}
