#![allow(dead_code)]
#![allow(unused_variables)]
// SPDX-FileCopyrightText: 2024 Julia DeMille <me@jdemille.com
//
// SPDX-License-Identifier: Parity-7.0.0

//! Parser and data structures for the X-Plane airways file.
//! Only `XPAWY1101`/`AwyXP1100` is supported.

use std::io::{BufRead, Read};

use itertools::Itertools;
use petgraph::Graph;
use snafu::ensure;
use winnow::{
    ascii::{dec_uint, space0, space1},
    combinator::{delimited, fail, preceded, separated, success},
    dispatch,
    token::any,
    trace::trace,
    PResult, Parser,
};

use crate::navdata::{
    match_wpt_predicate, parse_fixed_str, BadLastLineSnafu, Header, InvalidAwyDirSnafu,
    NavEdge, NavEntry, ParseError, ParseSnafu, ParsedNodeRef, ParsedNodeRefType,
    ReferencedNonexistentWptSnafu,
};

#[derive(Debug, Clone)]
pub struct AwyEdge {
    pub base_fl: u16,
    pub top_fl: u16,
    pub is_high: bool,
    pub name: heapless::String<5>,
}

pub(super) fn parse_file_buffered<F: Read + BufRead>(
    file: F,
    nav_graph: &mut Graph<NavEntry, NavEdge>,
) -> Result<Header, ParseError> {
    let mut lines = file.lines();
    let header = super::parse_header(|md_type| md_type == "AwyXP1100", &mut lines)?;
    let mut lines = lines
        .filter(|lin| lin.as_ref().map_or(true, |lin| !lin.is_empty()))
        .peekable();

    lines
        .peeking_take_while(|l| l.as_ref().map_or(true, |l| l != "99"))
        .try_for_each(|line| -> Result<(), ParseError> {
            let parsed_edge = parse_row.parse(&line?).map_err(|e| {
                ParseSnafu {
                    rendered: e.to_string(),
                    stage: "airway row",
                }
                .build()
            })?;
            let first_wpt_idx = nav_graph
                .node_indices()
                .find(match_wpt_predicate(&parsed_edge.first, nav_graph))
                .ok_or_else(|| {
                    ReferencedNonexistentWptSnafu {
                        wpt: parsed_edge.second.ident.to_string(),
                    }
                    .build()
                })?;
            let second_wpt_idx = nav_graph
                .node_indices()
                .find(match_wpt_predicate(&parsed_edge.second, nav_graph))
                .ok_or_else(|| {
                    ReferencedNonexistentWptSnafu {
                        wpt: parsed_edge.second.ident.to_string(),
                    }
                    .build()
                })?;
            for name in parsed_edge.names {
                let awy_edge = AwyEdge {
                    base_fl: parsed_edge.base_fl,
                    top_fl: parsed_edge.top_fl,
                    is_high: parsed_edge.is_high,
                    name,
                };
                ensure!(
                    matches!(parsed_edge.direction, 'B' | 'F' | 'N'),
                    InvalidAwyDirSnafu {
                        dir: parsed_edge.direction
                    }
                );
                if matches!(parsed_edge.direction, 'N' | 'F') {
                    nav_graph.add_edge(
                        first_wpt_idx,
                        second_wpt_idx,
                        NavEdge::Airway(awy_edge.clone()),
                    );
                }
                if matches!(parsed_edge.direction, 'N' | 'B') {
                    nav_graph.add_edge(
                        second_wpt_idx,
                        first_wpt_idx,
                        NavEdge::Airway(awy_edge.clone()),
                    );
                }
            }
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

struct ParsedAwyEdge {
    first: ParsedNodeRef,
    second: ParsedNodeRef,
    direction: char,
    is_high: bool,
    base_fl: u16,
    top_fl: u16,
    names: Vec<heapless::String<5>>,
}

fn parse_row(input: &mut &str) -> PResult<ParsedAwyEdge> {
    let first_ident =
        trace("first waypoint ident", parse_fixed_str::<5>).parse_next(input)?;
    let first_icao_region =
        trace("first waypoint ICAO region", parse_fixed_str::<2>).parse_next(input)?;
    let first_typ: ParsedNodeRefType = trace(
        "first waypoint type",
        dispatch! {preceded(space1, dec_uint);
            2 => success(ParsedNodeRefType::Vhf),
            3 => success(ParsedNodeRefType::Ndb),
            11 => success(ParsedNodeRefType::Fix),
            _ => fail
        },
    )
    .parse_next(input)?;

    let second_ident =
        trace("second waypoint ident", parse_fixed_str::<5>).parse_next(input)?;
    let second_icao_region =
        trace("second waypoint ICAO region", parse_fixed_str::<2>).parse_next(input)?;
    let second_typ: ParsedNodeRefType = trace(
        "second waypoint type",
        dispatch! {preceded(space1, dec_uint);
            2 => success(ParsedNodeRefType::Vhf),
            3 => success(ParsedNodeRefType::Ndb),
            11 => success(ParsedNodeRefType::Fix),
            _ => fail
        },
    )
    .parse_next(input)?;

    let direction: char = trace("direction", preceded(space1, any)).parse_next(input)?;
    let is_high = trace(
        "check high",
        dispatch! { preceded(space1, dec_uint::<_, u8, _>);
            1 => success(false),
            2 => success(true),
            _ => fail
        },
    )
    .parse_next(input)?;
    let base_fl: u16 = trace("base FL", preceded(space1, dec_uint)).parse_next(input)?;
    let top_fl: u16 = trace("top FL", preceded(space1, dec_uint)).parse_next(input)?;
    let names: Vec<heapless::String<5>> = trace(
        "section names",
        delimited(space1, separated(1.., parse_fixed_str::<5>, "-"), space0),
    )
    .parse_next(input)?;
    Ok(ParsedAwyEdge {
        first: ParsedNodeRef {
            ident: first_ident,
            icao_region: first_icao_region,
            typ: first_typ,
        },
        second: ParsedNodeRef {
            ident: second_ident,
            icao_region: second_icao_region,
            typ: second_typ,
        },
        direction,
        is_high,
        base_fl,
        top_fl,
        names,
    })
}
