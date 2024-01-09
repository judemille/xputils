// SPDX-FileCopyrightText: 2024 Julia DeMille <me@jdemille.com
//
// SPDX-License-Identifier: Parity-7.0.0

pub mod airways;
pub mod cifp;
pub mod fix;
pub mod hold;
pub mod nav;

#[cfg(feature = "RUSTC_IS_NIGHTLY")]
use const_format::concatcp;

use petgraph::graph::{DiGraph, NodeIndex};
use snafu::{prelude::*, Backtrace};
use std::{
    fs::File,
    io::{BufRead, BufReader, Error as IoError, Lines, Read},
    path::Path,
    rc::Rc,
};
use winnow::{
    ascii::{digit1, space0, space1},
    combinator::{fail, preceded, rest, success},
    dispatch,
    error::ContextError,
    prelude::*,
    stream::AsChar,
    token::{take, take_till1, take_until1},
    trace::trace,
};

use crate::navdata::{
    airways::AwyEdge,
    fix::Fix,
    hold::Edge as HoldEdge,
    nav::{Navaid, TypeSpecificData},
};

pub struct NavigationalData {
    fix_header: Header,
    navaids_header: Header,
    nav_graph: DiGraph<NavEntry, NavEdge>,
}

impl NavigationalData {
    /// Parses all navdata from the X-Plane `Custom Data` folder.
    /// # Errors
    /// Returns an [`Err`] if there is an I/O error, or if the data is malformed.
    pub fn build_data_from_folder(folder: &Path) -> Result<Self, ParseError> {
        let fix_file = BufReader::new(File::open(folder.join("earth_fix.dat"))?);
        let mut fixes = fix::parse_file_buffered(fix_file)?;
        let user_fixes = folder.join("user_fix.dat");
        if user_fixes.exists() {
            let user_fixes = BufReader::new(File::open(user_fixes)?);
            let user_fixes = fix::parse_file_buffered(user_fixes)?;
            for user_fix in user_fixes.entries {
                // Essentially, check if there is a fix in the same area, with the same ident.
                let matching_main_fix_pos = fixes.entries.iter().position(|fix| {
                    fix.ident == user_fix.ident
                        && fix.icao_region == user_fix.icao_region
                        && fix.terminal_region == user_fix.terminal_region
                });
                if let Some(pos) = matching_main_fix_pos {
                    fixes.entries[pos] = user_fix;
                } else {
                    fixes.entries.push(user_fix);
                }
            }
        }
        let nav_file = BufReader::new(File::open(folder.join("earth_nav.dat"))?);
        let mut navaids = nav::parse_file_buffered(nav_file)?;
        let established_cycle = fixes.header.cycle;
        ensure!(
            navaids.header.cycle == established_cycle,
            CycleMismatchSnafu {
                established_cycle,
                new_cycle: navaids.header.cycle
            }
        );
        let user_nav = folder.join("user_nav.dat");
        if user_nav.exists() {
            let user_nav = BufReader::new(File::open(user_nav)?);
            let user_nav = nav::parse_file_buffered(user_nav)?;
            for user_navaid in user_nav.entries {
                // Essentially, check if there is a matching navaid of the same type, in the same place, with the same ident.
                let matching_main_navaid_pos =
                    navaids.entries.iter().position(|navaid| {
                        navaid.ident == user_navaid.ident
                            && navaid.icao_region == user_navaid.icao_region
                            && std::mem::discriminant(&navaid.type_data)
                                == std::mem::discriminant(&user_navaid.type_data)
                    });
                if let Some(pos) = matching_main_navaid_pos {
                    navaids.entries[pos] = user_navaid;
                } else {
                    navaids.entries.push(user_navaid);
                }
            }
        }
        let fix_header = fixes.header;
        let navaids_header = navaids.header;
        let mut nav_graph = DiGraph::<NavEntry, NavEdge>::with_capacity(
            fixes.entries.len() + navaids.entries.len(),
            0,
        );
        for fix in fixes.entries {
            nav_graph.add_node(NavEntry::Fix(fix));
        }
        for navaid in navaids.entries {
            nav_graph.add_node(NavEntry::Navaid(navaid));
        }

        let airway_file = BufReader::new(File::open(folder.join("earth_awy.dat"))?);
        let airway_header = airways::parse_file_buffered(airway_file, &mut nav_graph)?;
        ensure!(
            airway_header.cycle == established_cycle,
            CycleMismatchSnafu {
                established_cycle,
                new_cycle: airway_header.cycle
            }
        );

        let hold_file = BufReader::new(File::open(folder.join("earth_hold.dat"))?);
        let hold_header = hold::parse_file_buffered(hold_file, &mut nav_graph)?;
        ensure!(
            hold_header.cycle == established_cycle,
            CycleMismatchSnafu {
                established_cycle,
                new_cycle: hold_header.cycle
            }
        );
        todo!()
    }

    #[must_use]
    /// Find all entries matching the given `ident` in the navigation database.
    /// Returns tuples of the indices of the nodes and references to the entries.
    pub fn find_nav_entry(&self, ident: &str) -> Vec<(NodeIndex, &NavEntry)> {
        self.nav_graph
            .node_indices()
            .filter(|idx| match &self.nav_graph[*idx] {
                NavEntry::Fix(fix) => fix.ident == ident,
                NavEntry::Navaid(navaid) => navaid.ident == ident,
            })
            .map(|idx| (idx, &self.nav_graph[idx]))
            .collect()
    }
}

#[derive(Debug, Clone)]
pub enum NavEntry {
    Fix(Fix),
    Navaid(Navaid),
}

#[derive(Debug, Clone)]
pub enum NavEdge {
    Airway(AwyEdge),
    Hold(HoldEdge),
}

#[derive(Debug, Copy, Clone)]
pub enum DataVersion {
    XP1100,
    XP1101,
    XP1140,
    XP1150,
    XP1200,
}

#[derive(Debug)]
pub struct Header {
    pub version: DataVersion,
    pub cycle: u16,
    pub build: u32,
    pub copyright: String,
}

#[derive(Debug, Snafu)]
pub enum ParseError {
    #[snafu(display("An I/O error has occurred!"))]
    #[snafu(context(false))]
    Io {
        source: IoError,
        backtrace: Backtrace,
    },
    #[snafu(display("Error occurred when parsing `{stage}`: \n\n{rendered}"))]
    Parse {
        rendered: String,
        stage: String,
        backtrace: Backtrace,
    },
    #[snafu(display("The byte order marker was an unexpected value: {bom}"))]
    BadBOM { bom: String, backtrace: Backtrace },
    #[snafu(display("The last line of this file was unexpected:\n{last_line}"))]
    BadLastLine {
        last_line: String,
        backtrace: Backtrace,
    },
    #[snafu(display("A line was expected, but the file had no more."))]
    MissingLine,
    #[snafu(display(
        "The data version {version:?} is not supported by the parser for this format."
    ))]
    UnsupportedVersion {
        version: DataVersion,
        backtrace: Backtrace,
    },
    #[snafu(display("The first navdata file parsed had AIRAC cycle {established_cycle}, but another file had {new_cycle}."))]
    CycleMismatch {
        established_cycle: u16,
        new_cycle: u16,
        backtrace: Backtrace,
    },
    #[snafu(display(
        "A navdata file referenced the waypoint {wpt}, which does not exist."
    ))]
    ReferencedNonexistentWpt { wpt: String, backtrace: Backtrace },
    #[snafu(display("An invalid airway direction was encountered: `{dir}`"))]
    InvalidAwyDir { dir: char, backtrace: Backtrace },
    #[snafu(display("An invalid hold direction was encountered: `{dir}`"))]
    InvalidHoldDir { dir: char, backtrace: Backtrace },
    #[snafu(display("A hold entry had conflicting hold leg lengths of `{minutes}` minutes, and `{dme}` DME."))]
    ConflictingHoldLegLengths {
        minutes: f32,
        dme: f32,
        backtrace: Backtrace,
    },
}

#[derive(Debug, Snafu)]
#[snafu(display("A string was too large."))]
struct StringTooLarge;

fn parse_header<F: Read + BufRead>(
    verify_type: impl Fn(&str) -> bool,
    lines: &mut Lines<F>,
) -> Result<Header, ParseError> {
    lines.next().ok_or(ParseError::MissingLine).and_then(|l| {
        let bom = l?;
        if bom != "A" && bom != "I" {
            return BadBOMSnafu { bom }.fail();
        }
        Ok(())
    })?;
    lines
        .next()
        .ok_or(ParseError::MissingLine)
        .and_then(|line| {
            let line = line?;
            let ret = parse_header_after_bom(verify_type)
                .parse(&line)
                .map_err(|e| {
                    ParseSnafu {
                        rendered: e.to_string(),
                        stage: "header",
                    }
                    .build()
                });
            ret // Weird lifetime error if I don't do this.
        })
}

fn parse_header_after_bom<'a>(
    verify_type: impl Fn(&str) -> bool,
) -> impl Parser<&'a str, Header, ContextError> {
    let verify_type = Rc::new(verify_type); // Gets rid of stupid lifetime errors.
    move |input: &mut &str| -> PResult<Header> {
        let version = trace(
            "get data version",
            dispatch! {take(4usize);
                "1100" => success(DataVersion::XP1100),
                "1101" => success(DataVersion::XP1101),
                "1140" => success(DataVersion::XP1140),
                "1150" => success(DataVersion::XP1150),
                "1200" => success(DataVersion::XP1200),
                _ => fail
            },
        )
        .parse_next(input)?;
        " Version - data cycle ".parse_next(input)?;
        let cycle: u16 = trace(
            "get cycle from header",
            take(4u8).and_then(digit1).try_map(|s: &str| s.parse()),
        )
        .parse_next(input)?;

        ", build ".parse_next(input)?;
        let build: u32 = trace(
            "get build from header",
            take(8u8).and_then(digit1).try_map(|s: &str| s.parse()),
        )
        .parse_next(input)?;

        ", metadata ".parse_next(input)?;
        trace(
            "verify header metadata type/version",
            take_until1(".").verify(&*verify_type),
        )
        .parse_next(input)?;
        '.'.parse_next(input)?;
        let copyright = trace("get header copyright", preceded(space0, rest))
            .parse_next(input)?
            .to_string();
        Ok(Header {
            version,
            cycle,
            build,
            copyright,
        })
    }
}

fn parse_fixed_str<const N: usize>(input: &mut &str) -> PResult<heapless::String<N>> {
    #[cfg(feature = "RUSTC_IS_NIGHTLY")]
    const TRACE_NOTE: &str = concatcp!("parse string of maximum length `", N, "`");
    #[cfg(not(feature = "RUSTC_IS_NIGHTLY"))]
    const TRACE_NOTE: &str = "parse string of fixed maximum length";
    trace(
        TRACE_NOTE,
        preceded(space1, take_till1(|c: char| c.is_space())).try_map(|id: &str| {
            heapless::String::<N>::try_from(id).map_err(|()| StringTooLarge)
        }),
    )
    .parse_next(input)
}

fn match_wpt_predicate<'a>(
    wpt: &'a ParsedNodeRef,
    nav_graph: &'a DiGraph<NavEntry, NavEdge>,
) -> impl Fn(&NodeIndex) -> bool + 'a {
    |idx| -> bool {
        match (wpt.typ, &nav_graph[*idx]) {
            (ParsedNodeRefType::Fix, NavEntry::Fix(fix)) => {
                wpt.ident == fix.ident && wpt.icao_region == fix.icao_region
            },
            (ParsedNodeRefType::Vhf, NavEntry::Navaid(navaid)) => {
                wpt.ident == navaid.ident
                    && wpt.icao_region == navaid.icao_region
                    && matches!(
                        navaid.type_data,
                        TypeSpecificData::Vor { .. }
                            | TypeSpecificData::Dme {
                                display_freq: true,
                                ..
                            }
                    )
            },
            (ParsedNodeRefType::Ndb, NavEntry::Navaid(navaid)) => {
                wpt.ident == navaid.ident
                    && wpt.icao_region == navaid.icao_region
                    && matches!(navaid.type_data, TypeSpecificData::Ndb { .. })
            },
            _ => false,
        }
    }
}

struct ParsedNodeRef {
    ident: heapless::String<5>,
    icao_region: heapless::String<2>,
    typ: ParsedNodeRefType,
}

#[derive(Debug, Copy, Clone)]
enum ParsedNodeRefType {
    Ndb,
    Vhf,
    Fix,
}
