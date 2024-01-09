// SPDX-FileCopyrightText: 2024 Julia DeMille <me@jdemille.com
//
// SPDX-License-Identifier: Parity-7.0.0

use std::io::{BufRead, Read};

use itertools::Itertools;
use petgraph::graph::DiGraph;
use snafu::ensure;
use winnow::{
    ascii::{dec_uint, float, space1},
    combinator::{fail, preceded, success},
    dispatch,
    token::any,
    trace::trace,
    PResult, Parser,
};

use crate::navdata::{
    match_wpt_predicate,
    nav::{Navaid, TypeSpecificData},
    parse_fixed_str, BadLastLineSnafu, ConflictingHoldLegLengthsSnafu, DataVersion,
    Header, InvalidHoldDirSnafu, NavEdge, NavEntry, ParseError, ParseSnafu,
    ParsedNodeRef, ParsedNodeRefType, ReferencedNonexistentWptSnafu,
    UnsupportedVersionSnafu,
};

#[derive(Debug, Clone)]
pub struct Edge {
    pub inbound_crs_mag: f32,
    pub leg_length: LegLength,
    pub turn_direction: Direction,
    pub min_alt_ft: Option<u32>,
    pub max_alt_ft: Option<u32>,
    /// If [`None`], ICAO rules apply.
    pub max_spd_kts: Option<u16>,
}

#[derive(Debug, Clone, Copy)]
pub enum LegLength {
    Minutes(f32),
    DME(f32),
}

#[derive(Debug, Clone, Copy)]
pub enum Direction {
    Left,
    Right,
}

#[allow(clippy::too_many_lines)]
pub(super) fn parse_file_buffered<F: Read + BufRead>(
    file: F,
    nav_graph: &mut DiGraph<NavEntry, NavEdge>,
) -> Result<Header, ParseError> {
    let mut lines = file.lines();
    let header = super::parse_header(|md_type| md_type == "HoldXP1140", &mut lines)?;

    ensure!(
        matches!(header.version, DataVersion::XP1140),
        UnsupportedVersionSnafu {
            version: header.version
        }
    );

    let mut lines = lines
        .filter(|lin| lin.as_ref().map_or(true, |lin| !lin.is_empty()))
        .peekable();

    lines
        .peeking_take_while(|l| l.as_ref().map_or(true, |l| l != "99"))
        .try_for_each(|line| -> Result<(), ParseError> {
            let line = line?;
            let parsed_edge = trace("hold row", parse_row).parse(&line).map_err(|e| {
                ParseSnafu {
                    rendered: e.to_string(),
                    stage: "hold row",
                }
                .build()
            })?;

            let hold_point_idx = nav_graph
                .node_indices()
                .filter(|idx| match &nav_graph[*idx] {
                    NavEntry::Fix(fix) => {
                        fix.terminal_region == parsed_edge.terminal_region
                    },
                    NavEntry::Navaid(Navaid {
                        type_data: TypeSpecificData::Vor { .. },
                        ..
                    }) => parsed_edge.terminal_region == "ENRT",
                    NavEntry::Navaid(Navaid {
                        type_data:
                            TypeSpecificData::Ndb {
                                terminal_region, ..
                            }
                            | TypeSpecificData::Dme {
                                terminal_region, ..
                            },
                        ..
                    }) => terminal_region == &parsed_edge.terminal_region,
                    NavEntry::Navaid(_) => false,
                })
                .find(match_wpt_predicate(&parsed_edge.hold_point, nav_graph))
                .ok_or_else(|| {
                    ReferencedNonexistentWptSnafu {
                        wpt: parsed_edge.hold_point.ident.to_string(),
                    }
                    .build()
                })?;

            let turn_direction = match parsed_edge.direction {
                'L' => Direction::Left,
                'R' => Direction::Right,
                _ => {
                    return InvalidHoldDirSnafu {
                        dir: parsed_edge.direction,
                    }
                    .fail()
                },
            };

            #[allow(illegal_floating_point_literal_pattern)]
            let leg_length = match (parsed_edge.leg_time_min, parsed_edge.leg_length_nm) {
                (minutes, 0f32) => LegLength::Minutes(minutes),
                (0f32, dme) => LegLength::DME(dme),
                (minutes, dme) => {
                    return ConflictingHoldLegLengthsSnafu { minutes, dme }.fail()
                },
            };

            let min_alt_ft = if parsed_edge.min_alt_ft == 0 {
                None
            } else {
                Some(parsed_edge.min_alt_ft)
            };

            let max_alt_ft = if parsed_edge.max_alt_ft == 0 {
                None
            } else {
                Some(parsed_edge.max_alt_ft)
            };

            let max_spd_kts = if parsed_edge.max_spd_kts == 0 {
                None
            } else {
                Some(parsed_edge.max_spd_kts)
            };

            let edge = Edge {
                inbound_crs_mag: parsed_edge.inbound_crs_mag,
                leg_length,
                turn_direction,
                min_alt_ft,
                max_alt_ft,
                max_spd_kts,
            };

            nav_graph.add_edge(hold_point_idx, hold_point_idx, NavEdge::Hold(edge));

            Ok(())
        })?;

    lines
        .next()
        .ok_or_else(|| ParseError::MissingLine)
        .and_then(|last_line| {
            let last_line = last_line?;
            ensure!(last_line == "99", BadLastLineSnafu { last_line });
            Ok(())
        })?;

    Ok(header)
}

struct ParsedEdge {
    hold_point: ParsedNodeRef,
    terminal_region: heapless::String<4>,
    inbound_crs_mag: f32,
    leg_time_min: f32,
    leg_length_nm: f32,
    direction: char,
    min_alt_ft: u32,
    max_alt_ft: u32,
    max_spd_kts: u16,
}

fn parse_row(input: &mut &str) -> PResult<ParsedEdge> {
    let hold_point_ident = trace("ident", parse_fixed_str::<5>).parse_next(input)?;
    let icao_region =
        trace("ICAO region code", parse_fixed_str::<2>).parse_next(input)?;
    let terminal_region =
        trace("terminal region", parse_fixed_str::<4>).parse_next(input)?;
    let point_typ: ParsedNodeRefType = trace(
        "point type",
        dispatch! { preceded(space1, dec_uint);
            2 => success(ParsedNodeRefType::Vhf),
            3 => success(ParsedNodeRefType::Ndb),
            11 => success(ParsedNodeRefType::Fix),
            _ => fail,
        },
    )
    .parse_next(input)?;

    let inbound_crs_mag: f32 =
        trace("inbound course, magnetic degrees", preceded(space1, float))
            .parse_next(input)?;

    let leg_time_min: f32 =
        trace("leg time, minutes", preceded(space1, float)).parse_next(input)?;
    let leg_length_nm: f32 =
        trace("leg length, NM", preceded(space1, float)).parse_next(input)?;

    let direction: char = trace("direction", preceded(space1, any)).parse_next(input)?;

    let min_alt_ft: u32 =
        trace("minimum altitude, ft", preceded(space1, dec_uint)).parse_next(input)?;
    let max_alt_ft: u32 =
        trace("maximum altitude, ft", preceded(space1, dec_uint)).parse_next(input)?;

    let max_spd_kts: u16 =
        trace("maximum speed, NM/h", preceded(space1, dec_uint)).parse_next(input)?;
    Ok(ParsedEdge {
        hold_point: ParsedNodeRef {
            ident: hold_point_ident,
            icao_region,
            typ: point_typ,
        },
        terminal_region,
        inbound_crs_mag,
        leg_time_min,
        leg_length_nm,
        direction,
        min_alt_ft,
        max_alt_ft,
        max_spd_kts,
    })
}
