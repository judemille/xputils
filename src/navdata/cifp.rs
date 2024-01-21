// SPDX-FileCopyrightText: 2024 Julia DeMille <me@jdemille.com>
//
// SPDX-License-Identifier: Parity-7.0.0

use std::str::FromStr;

use winnow::{
    ascii::{alpha1, dec_int, dec_uint, float, space0},
    combinator::{dispatch, fail, opt, rest, seq, terminated},
    prelude::*,
    stream::AsChar,
    token::{none_of, take_until0},
    trace::trace,
    Located,
};

use heapless::String as HString;

use crate::navdata::{fixed_hstring_till, take_hstring_till};

#[derive(Debug, Clone)]
enum Row {
    Sid(Box<SidStarApchRow>),
    Star(Box<SidStarApchRow>),
    Apch(Box<SidStarApchRow>),
    Rwy(Box<RwyRow>),
    /// Cannot find *any* information on how PRDAT rows work. Reading the files isn't
    /// any use either. Just ignoring for now.
    PrDat,
}

#[derive(Debug, Clone)]
struct SidStarApchRow {
    sequence: u16,
    route_typ: char,
    proc_ident: HString<6>,
    trans_ident: Option<HString<5>>,
    wpt_ident: Option<HString<5>>,
    wpt_icao_region: Option<HString<2>>,
    section: Option<char>,
    subsection: Option<char>,
    waypoint_desc_code: Option<HString<4>>,
    turn_dir: Option<char>,
    rnp: Option<f32>,
    path_and_term: Option<HString<2>>,
    // Once again: What the fuck, ARINC?
    // This isn't even about the turn direction being *valid*, it's about it being
    // required prior to capturing the path.
    turn_dir_valid: Option<char>,
    rcmd_navaid: Option<HString<4>>,
    rcmd_navaid_icao_region: Option<HString<2>>,
    rcmd_navaid_section: Option<char>,
    rcmd_navaid_subsection: Option<char>,
    arc_radius_nm: Option<f64>,
    theta: Option<f64>,
    rho: Option<f64>,
    // Except when it isn't magnetic!
    ob_mag_crs: Option<HString<4>>,
    // Fuck you, ARINC!
    rte_dist_from_or_hold_dist_time: Option<HString<4>>,
    alt_desc: Option<char>,
    alt_one: Option<HString<5>>,
    alt_two: Option<HString<5>>,
    trans_alt_ft_msl: Option<u32>,
    speed_lim_desc: Option<char>,
    speed_lim: Option<u16>,
    // :ferrisEvil:
    vertical_angle: Option<f32>,

    // Column that I don't have a reference for. Look, I only have ARINC 424-17.
    // Reference 5.293, in case anyone knows.

    // Once again, ARINC: Fuck you!
    center_fix_or_proc_turn: Option<HString<5>>,
    center_fix_icao_region: Option<HString<2>>,
    center_fix_section: Option<char>,
    center_fix_subsection: Option<char>,
    multiple_code_or_taa_sect_ident: Option<char>,
    gps_fms_indicator: Option<char>,
    rte_qual1: Option<char>,
    rte_qual2: Option<char>,
}

#[derive(Debug, Clone)]
struct RwyRow {
    rwy_ident: HString<5>,
    rwy_grad_1_1000_pct: Option<i16>,
    ellipsoidal_height_1_10m: Option<i64>,
    landing_threshold_elev_ft_msl: i64,
    tch_val_indicator: Option<char>,
    loc_mls_gls_ident: Option<HString<4>>,
    ils_mls_gls_cat: Option<char>,
    thresh_cross_height_ft_agl: Option<u8>,
    lat: HString<10>,
    lon: HString<10>,
    displaced_thresh_dist_ft: u16,
}

fn parse_row(input: &mut Located<&str>) -> PResult<Row> {
    dispatch! {terminated(alpha1, ':');
        "SID" => parse_ssa_row.map(Row::Sid),
        "STAR" => parse_ssa_row.map(Row::Star),
        "APPCH" => parse_ssa_row.map(Row::Apch),
        "RWY" => parse_rwy_row.map(Row::Rwy),
        // See [`Row::PrDat`].
        "PRDAT" => rest.map(|_| Row::PrDat),
        _ => fail
    }
    .parse_next(input)
}

// Helper function for row parsing.
fn comma(c: char) -> bool {
    c == ','
}

fn handle_empty<const N: usize>(s: HString<N>) -> Option<HString<N>> {
    if s.chars().any(|c| !c.is_space()) {
        Some(s)
    } else {
        None
    }
}

fn parse_ssa_row(input: &mut Located<&str>) -> PResult<Box<SidStarApchRow>> {
    seq! {
        SidStarApchRow {
            sequence: trace("sequence", dec_uint),
            _: (space0, ','),
            route_typ: trace("route type", none_of(',')),
            _: (space0, ','),
            proc_ident: trace("procedure ident", take_hstring_till(comma)),
            _: (space0, ','),
            trans_ident: trace("transition ident", take_hstring_till(comma)).map(handle_empty),
            _: (space0, ','),
            wpt_ident: trace("waypoint ident", take_hstring_till(comma)).map(handle_empty),
            _: (space0, ','),
            wpt_icao_region: trace("waypoint ICAO region", take_hstring_till(comma)).map(handle_empty),
            _: (space0, ','),
            section: trace("waypoint data section", opt(none_of([' ', ',']))),
            _: (space0, ','),
            subsection: trace("waypoint data subsection", opt(none_of([' ', ',']))),
            _: (space0, ','),
            waypoint_desc_code: trace("waypoint description code", fixed_hstring_till(comma)).map(handle_empty),
            _: (space0, ','),
            turn_dir: trace("turn direction", opt(none_of([' ', ',']))),
            _: (space0, ','),
            // Why, ARINC, why?
            rnp: trace("RNP",
                    opt(fixed_hstring_till::<3, _>(comma)
                        .verify(|s| s.len() == 3)
                        .try_map(|rnp_str| -> Result<f32, <f32 as FromStr>::Err> {
                            let (significand, exponent) = rnp_str.split_at(2);
                            let (significand, exponent) = (significand.parse::<f32>()?, exponent.parse::<f32>()?);
                            Ok(significand * (10f32.powf(exponent * -1f32)))
            }))),
            _: (space0, ','),
            path_and_term: trace("path and term.", fixed_hstring_till(comma)).map(handle_empty),
            _: (space0, ','),
            turn_dir_valid: trace("turn dir valid", opt(none_of([' ', ',']))),
            _: (space0, ','),
            rcmd_navaid: trace("recommended navaid", take_hstring_till(comma)).map(handle_empty),
            _: (space0, ','),
            rcmd_navaid_icao_region: trace("rcmd navaid ICAO reg", take_hstring_till(comma)).map(handle_empty),
            _: (space0, ','),
            rcmd_navaid_section: trace("rcmd navaid data section", opt(none_of([' ', ',']))),
            _: (space0, ','),
            rcmd_navaid_subsection: trace("rcmd navaid data subsection", opt(none_of([' ', ',']))),
            _: (space0, ','),
            arc_radius_nm: trace("arc radius, 1/1000 nm",
                opt(float)
                .map(|aro| aro.map(|ar: f64| ar * 1000f64))
            ),
            _: (space0, ','),
            theta: trace("θ, 1/10°",
                opt(float)
                .map(|th| th.map(|th: f64| th * 10f64))
            ),
            _: (space0, ','),
            rho: trace("ρ, 1/10nm",
                opt(float)
                .map(|rho| rho.map(|rho: f64| rho * 10f64))
            ),
            _: (space0, ','),
            ob_mag_crs: trace("outbound magnetic course", fixed_hstring_till(comma)).map(handle_empty),
            _: (space0, ','),
            rte_dist_from_or_hold_dist_time: trace("rte dist. from/hold dist/time", fixed_hstring_till(comma)).map(handle_empty),
            _: (space0, ','),
            alt_desc: trace("altitude descriptor", opt(none_of([' ', ',']))),
            _: (space0, ','),
            alt_one: trace("altitude one", fixed_hstring_till(comma)).map(handle_empty),
            _: (space0, ','),
            alt_two: trace("altitude two", fixed_hstring_till(comma)).map(handle_empty),
            _: (space0, ','),
            trans_alt_ft_msl: trace("transition altitude, ft MSL", opt(dec_uint)),
            _: (space0, ','),
            speed_lim_desc: trace("speed limit descriptor", opt(none_of([' ', ',']))),
            _: (space0, ','),
            speed_lim: trace("speed limit", opt(dec_uint)),
            _: (space0, ','),
            vertical_angle: trace("vertical angle", opt(float.map(|va: f32| va / 100f32))),
            _: (space0, ',', trace("discarding unknown column 5.293", take_until0(',')), ',', space0),
            center_fix_or_proc_turn: take_hstring_till(comma).map(handle_empty),
            _: (space0, ','),
            center_fix_icao_region: take_hstring_till(comma).map(handle_empty),
            _: (space0, ','),
            center_fix_section: trace("center fix data section", opt(none_of([' ', ',']))),
            _: (space0, ','),
            center_fix_subsection: trace("center fix data subsection", opt(none_of([' ', ',']))),
            _: (space0, ','),
            multiple_code_or_taa_sect_ident: trace("multiple code/TAA sect. ident", opt(none_of([' ', ',']))),
            _: (space0, ','),
            gps_fms_indicator: trace("GPS/FMS indicator", opt(none_of([' ', ',']))),
            _: (space0, ','),
            rte_qual1: trace("rte qual 1", opt(none_of([' ', ',']))),
            _: (space0, ','),
            rte_qual2: trace("rte qual 2", opt(none_of([' ', ',']))),
            _: (space0, trace("line ending", ';')),
        }
    }
    .parse_next(input)
    .map(Box::new)
}

fn parse_rwy_row(input: &mut Located<&str>) -> PResult<Box<RwyRow>> {
    seq! {
        RwyRow {
            rwy_ident: take_hstring_till(comma),
            _: (space0, ','),
            rwy_grad_1_1000_pct: opt(dec_int),
            _: (space0, ','),
            ellipsoidal_height_1_10m: opt(dec_int),
            _: (space0, ','),
            landing_threshold_elev_ft_msl: dec_int,
            _: (space0, ','),
            tch_val_indicator: opt(none_of([' ', ','])),
            _: (space0, ','),
            loc_mls_gls_ident: take_hstring_till(comma).map(handle_empty),
            _: (space0, ','),
            ils_mls_gls_cat: opt(none_of([' ', ','])),
            _: (space0, ','),
            thresh_cross_height_ft_agl: opt(dec_uint),
            _: (space0, ';'),
            lat: take_hstring_till(comma),
            _: (space0, ','),
            lon: take_hstring_till(comma),
            _: (space0, ','),
            displaced_thresh_dist_ft: dec_uint,
            _: (space0, trace("line ending", ';')),
        }
    }
    .parse_next(input)
    .map(Box::new)
}

#[cfg(test)]
mod tests {
    use std::{
        fs::File,
        io::{BufRead, BufReader},
        path::Path,
    };

    use snafu::{OptionExt, Report, ResultExt, Whatever};
    use winnow::{Located, Parser};

    use crate::navdata::cifp::{parse_row, Row};

    #[test]
    fn parse_a_bunch_of_rows() -> Report<Whatever> {
        Report::capture(|| {
            let cifp_dir = Path::new(file!())
                .parent() // src/navdata
                .whatever_context("cifp.rs has no parent???")?
                .parent() // src
                .whatever_context("src/navdata has no parent???")?
                .parent()
                .whatever_context("src has no parent???")?
                .join("xp_nav/CIFP");

            let ksfo_dat = BufReader::new(
                File::open(cifp_dir.join("KSFO.dat"))
                    .whatever_context("could not open xp_nav/CIFP/KSFO.dat")?,
            );
            let ksfo_rows: Vec<_> = ksfo_dat
                .lines()
                .collect::<Result<Vec<_>, _>>()
                .whatever_context("reading a line from KSFO.dat failed")?
                .into_iter()
                .map(|line| {
                    parse_row
                        .parse(Located::new(&line))
                        .map_err(|e| e.to_string())
                        .whatever_context("failed to parse a row in KSFO.dat")
                })
                .collect::<Result<Vec<_>, _>>()?;

            println!(
                "first 4 KSFO SID rows: {:#?}\n\n",
                ksfo_rows
                    .iter()
                    .filter(|row| matches!(row, Row::Sid(_)))
                    .take(4)
                    .collect::<Vec<_>>()
            );
            println!(
                "first 4 KSFO STAR rows: {:#?}\n\n",
                ksfo_rows
                    .iter()
                    .filter(|row| matches!(row, Row::Star(_)))
                    .take(4)
                    .collect::<Vec<_>>()
            );
            println!(
                "first 4 KSFO APPCH rows: {:#?}\n\n",
                ksfo_rows
                    .iter()
                    .filter(|row| matches!(row, Row::Apch(_)))
                    .take(4)
                    .collect::<Vec<_>>()
            );
            println!(
                "first 4 KSFO RWY rows: {:#?}\n\n",
                ksfo_rows
                    .iter()
                    .filter(|row| matches!(row, Row::Rwy(_)))
                    .take(4)
                    .collect::<Vec<_>>()
            );

            Ok(())
        })
    }
}
