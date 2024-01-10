// SPDX-FileCopyrightText: 2024 Julia DeMille <me@jdemille.com
//
// SPDX-License-Identifier: Parity-7.0.0

use winnow::{
    ascii::{alpha1, dec_int, dec_uint, float, space0},
    combinator::{dispatch, fail, opt, rest, seq, terminated, todo},
    prelude::*,
    token::{any, take_until0},
};

use heapless::String as HString;

use crate::navdata::parse_fixed_str;

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
    trans_ident: HString<5>,
    wpt_ident: HString<5>,
    icao_region: HString<2>,
    section: char,
    subsection: char,
    waypoint_desc_code: HString<4>,
    turn_dir: char,
    // Why, ARINC, why?
    rnp: HString<5>,
    path_and_term: HString<2>,
    // Once again: What the fuck, ARINC?
    // This isn't even about the turn direction being *valid*, it's about it being
    // required prior to capturing the path.
    turn_dir_valid: char,
    rcmd_navaid: HString<4>,
    rcmd_navaid_icao_region: HString<2>,
    rcmd_navaid_section: char,
    rcmd_navaid_subsection: char,
    // Why??????
    arc_radius_1_1000nm: u64,
    theta_1_10deg: u16,
    rho_1_10nm: u16,
    // Except when it isn't magnetic!
    ob_mag_crs: HString<4>,
    // Fuck you, ARINC!
    rte_dist_from_or_hold_dist_time: HString<4>,
    alt_desc: char,
    alt_one: HString<5>,
    alt_two: HString<5>,
    trans_alt_ft_msl: Option<u32>,
    speed_lim_desc: char,
    speed_lim: u16,
    // :ferrisEvil:
    vertical_angle: f32,

    // Column that I don't have a reference for. Look, I only have ARINC 424-17.
    // Reference 5.293, in case anyone knows.

    // Once again, ARINC: Fuck you!
    center_fix_or_proc_turn: HString<4>,
    center_fix_icao_region: HString<2>,
    center_fix_section: char,
    center_fix_subsection: char,
    multiple_code_or_taa_sect_ident: char,
    gps_fms_indicator: char,
    rte_qual1: char,
    rte_qual2: char,
}

#[derive(Debug, Clone)]
struct RwyRow {
    rwy_ident: HString<3>,
    rwy_grad_1_1000_pct: i16,
    ellipsoidal_height_1_10m: i64,
    landing_threshold_elev_ft_msl: i64,
    tch_val_indicator: char,
    llz_ident: HString<4>,
    ils_mls_gls_cat: char,
    thresh_cross_height_ft_agl: Option<u8>,
    lat: HString<9>,
    lon: HString<9>,
    displaced_thresh_dist_ft: u16,
}

fn parse_row(input: &mut &str) -> PResult<Row> {
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

fn parse_ssa_row(input: &mut &str) -> PResult<Box<SidStarApchRow>> {
    seq! {
        SidStarApchRow{
            sequence: dec_uint,
            _: (space0, ','),
            route_typ: any,
            _: (space0, ','),
            proc_ident: parse_fixed_str::<6>,
            _: (space0, ','),
            trans_ident: parse_fixed_str::<5>,
            _: (space0, ','),
            wpt_ident: parse_fixed_str::<5>,
            _: (space0, ','),
            icao_region: parse_fixed_str::<2>,
            _: (space0, ','),
            section: any,
            _: (space0, ','),
            subsection: any,
            _: (space0, ','),
            waypoint_desc_code: parse_fixed_str::<4>,
            _: (',', space0),
            turn_dir: any,
            _: (space0, ','),
            rnp: parse_fixed_str::<5>,
            _: (space0, ','),
            path_and_term: parse_fixed_str::<2>,
            _: (space0, ','),
            turn_dir_valid: any,
            _: (space0, ','),
            rcmd_navaid: parse_fixed_str::<4>,
            _: (space0, ','),
            rcmd_navaid_icao_region: parse_fixed_str::<2>,
            _: (space0, ','),
            rcmd_navaid_section: any,
            _: (space0, ','),
            rcmd_navaid_subsection: any,
            _: (space0, ','),
            arc_radius_1_1000nm: dec_uint,
            _: (space0, ','),
            theta_1_10deg: dec_uint,
            _: (space0, ','),
            rho_1_10nm: dec_uint,
            _: (space0, ','),
            ob_mag_crs: parse_fixed_str::<4>,
            _: (space0, ','),
            rte_dist_from_or_hold_dist_time: parse_fixed_str::<4>,
            _: (space0, ','),
            alt_desc: any,
            _: (space0, ','),
            alt_one: parse_fixed_str::<5>,
            _: (space0, ','),
            alt_two: parse_fixed_str::<5>,
            _: (space0, ','),
            trans_alt_ft_msl: opt(dec_uint),
            _: (space0, ','),
            speed_lim_desc: any,
            _: (space0, ','),
            speed_lim: dec_uint,
            _: (space0, ','),
            vertical_angle: float,
            _: (space0, ',', take_until0(','), space0),
            center_fix_or_proc_turn: parse_fixed_str::<4>,
            _: (space0, ','),
            center_fix_icao_region: parse_fixed_str::<2>,
            _: (space0, ','),
            center_fix_section: any,
            _: (space0, ','),
            center_fix_subsection: any,
            _: (space0, ','),
            multiple_code_or_taa_sect_ident: any,
            _: (space0, ','),
            gps_fms_indicator: any,
            _: (space0, ','),
            rte_qual1: any,
            _: (space0, ','),
            rte_qual2: any,
            _: (space0, ';'),
        }
    }
    .parse_next(input)
    .map(Box::new)
}

fn parse_rwy_row(input: &mut &str) -> PResult<Box<RwyRow>> {
    seq! {
        RwyRow {
            rwy_ident: parse_fixed_str::<3>,
            _: (space0, ','),
            rwy_grad_1_1000_pct: dec_int,
            _: (space0, ','),
            ellipsoidal_height_1_10m: dec_int,
            _: (space0, ','),
            landing_threshold_elev_ft_msl: dec_int,
            _: (space0, ','),
            tch_val_indicator: any,
            _: (space0, ','),
            llz_ident: parse_fixed_str::<4>,
            _: (space0, ','),
            ils_mls_gls_cat: any,
            _: (space0, ','),
            thresh_cross_height_ft_agl: opt(dec_uint),
            _: (space0, ';'),
            lat: parse_fixed_str::<9>,
            _: (space0, ','),
            lon: parse_fixed_str::<9>,
            _: (space0, ','),
            displaced_thresh_dist_ft: dec_uint,
            _: (space0, ';'),
        }
    }
    .parse_next(input)
    .map(Box::new)
}
