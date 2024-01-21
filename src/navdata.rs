// SPDX-FileCopyrightText: 2024 Julia DeMille <me@jdemille.com>
//
// SPDX-License-Identifier: Parity-7.0.0

pub mod airways;
pub mod cifp;
pub mod fix;
pub mod hold;
pub mod nav;

use either::Either::{self, Left, Right};
use petgraph::{
    graph::{DiGraph, NodeIndex},
    visit::{DfsPostOrder, EdgeFiltered, Walker},
};
use snafu::{prelude::*, Backtrace};
use std::{
    fmt::Display,
    fs::File,
    io::{BufRead, BufReader, Error as IoError, Lines, Read},
    path::Path,
    rc::Rc,
    str::FromStr,
};
use winnow::{
    ascii::{digit1, space0},
    combinator::{cut_err, fail, preceded, rest, success},
    dispatch,
    error::{ContextError, StrContext::Expected, StrContextValue::Description},
    prelude::*,
    token::{take, take_till, take_until1},
    trace::trace,
    Located,
};

use chumsky::{
    extra::{Full, ParserExtra},
    prelude::*,
    text::newline,
    Parser as CParser,
};

use crate::navdata::{
    airways::AwyEdge,
    fix::Fix,
    hold::Edge as HoldEdge,
    nav::{Navaid, TypeSpecificData},
};

pub struct NavGraph {
    fix_header: Header,
    navaids_header: Header,
    graph: DiGraph<NavEntry, NavEdge>,
}

impl NavGraph {
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
        let airway_header =
            airways::parse_file_buffered(airway_file, &mut nav_graph)?;
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
    /// Get a reference to the graph.
    /// Have fun.
    /// In all seriousness, if something has caused you to need access to the raw
    /// graph, you're either doing something wrong, or you should send an email to
    /// the list to request functionality. xputils should expose all needed functionality
    /// such that this is never required.
    pub fn graph(&self) -> &DiGraph<NavEntry, NavEdge> {
        &self.graph
    }

    #[must_use]
    /// Find all entries matching the given `ident` in the navigation database.
    /// Returns tuples of the indices of the nodes and references to the entries.
    pub fn find_nav_entry(&self, ident: &str) -> Vec<(NodeIndex, &NavEntry)> {
        self.graph
            .node_indices()
            .filter(|idx| match &self.graph[*idx] {
                NavEntry::Fix(fix) => fix.ident == ident,
                NavEntry::Navaid(navaid) => navaid.ident == ident,
            })
            .map(|idx| (idx, &self.graph[idx]))
            .collect()
    }

    /// Traverse the graph, starting at `start`, following the airway `awy` in
    /// either direction, searching for nodes matching `end`.
    ///
    /// If multiple nodes are returned, there are multiple nodes on the airway
    /// with that ident.
    ///
    /// # Errors
    /// An error will be returned if one of the following occurs:
    /// - A bad node index is given.
    /// - The starting node is not on the given airway.
    pub fn airway_find(
        &self,
        start: NodeIndex,
        awy: &str,
        end: &str,
    ) -> Result<Vec<(NodeIndex, &NavEntry)>, AirwayTraverseError> {
        if !self.graph.node_indices().any(|idx| idx == start) {
            return BadNodeSnafu { idx: start }.fail()?;
        }

        if !self
            .graph
            .edges(start)
            .any(|e| matches!(e.weight(), NavEdge::Airway(AwyEdge{name, ..}) if name == awy))
        {
            return NotOnAirwaySnafu {
                node: Left(start),
                awy: awy.to_owned(),
                start: true,
            }
            .fail();
        }

        let ef = EdgeFiltered::from_fn(
            &self.graph,
            |er| matches!(er.weight(), NavEdge::Airway(AwyEdge { name, .. }) if name == awy),
        );

        let res: Vec<_> = DfsPostOrder::new(&ef, start)
            .iter(&ef)
            .filter(|idx| match &self.graph[*idx] {
                NavEntry::Fix(Fix { ident, .. }) => ident == end,
                NavEntry::Navaid(Navaid { ident, .. }) => ident == end,
            })
            .map(|idx| (idx, &self.graph[idx]))
            .collect();

        if res.is_empty() {
            NotOnAirwaySnafu {
                node: Right(end.to_owned()),
                awy: awy.to_owned(),
                start: false,
            }
            .fail()
        } else {
            Ok(res)
        }
    }
}

#[derive(Debug, Snafu)]
pub enum AirwayTraverseError {
    /// The node `node` is not on the airway.
    ///
    /// Well, if `start` is false, it's possible it's just not reachable on this
    /// airway from the starting point. That is effectively equivalent for an FMS,
    /// though.
    #[snafu(display("The node {node:?} is not on airway {awy}."))]
    NotOnAirway {
        /// The offending node.
        node: Either<NodeIndex, String>,
        /// The airway in question.
        awy: String,
        /// Whether this node is the start of the search, or the end.
        /// May not be relevant, depending on your search.
        start: bool,
        backtrace: Backtrace,
    },
    #[snafu(display(
        "The traversal could not find a valid path to the node {idx:?}"
    ))]
    NoPath {
        idx: Either<NodeIndex, String>,
        backtrace: Backtrace,
    },
    #[snafu(context(false))]
    #[snafu(display("A graph error has been raised: {source:?}"))]
    Graph {
        source: GraphError,
        backtrace: Backtrace,
    },
}

#[derive(Debug, Snafu)]
pub enum GraphError {
    #[snafu(display("A bad node index has been given: {idx:?}"))]
    BadNode {
        /// The offending node index.
        idx: NodeIndex,
        backtrace: Backtrace,
    },
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
            let ret =
                parse_header_after_bom(verify_type)
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
) -> impl winnow::Parser<&'a str, Header, ContextError> {
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
        .context(Expected(Description("4-digit cycle number")))
        .parse_next(input)?;

        ", build ".parse_next(input)?;
        let build: u32 = trace(
            "get build from header",
            take(8u8).and_then(digit1).try_map(|s: &str| s.parse()),
        )
        .context(Expected(Description("8-digit build number")))
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

type VerifyStr<'a> =
    dyn Fn(&'a str, <&'a str as Input<'a>>::Span) -> Result<&'a str, Rich<'a, char>>;

fn parse_header_c<'a>(
    verify_type: Rc<VerifyStr<'a>>,
) -> impl CParser<'a, &'a str, Header, extra::Err<Rich<'a, char>>> + Clone {
    let bom =
        chumsky::primitive::one_of::<_, &'a str, extra::Err<Rich<'a, char>>>("IA");

    let data_ver = chum_uint::<u32>(None)
        .try_map(|v, span| match v {
            1100 => Ok(DataVersion::XP1100),
            1101 => Ok(DataVersion::XP1101),
            1140 => Ok(DataVersion::XP1140),
            1150 => Ok(DataVersion::XP1150),
            1200 => Ok(DataVersion::XP1200),
            _ => Err(Rich::custom(
                span,
                format!("unrecognized data version `{v}`"),
            )),
        })
        .labelled("data version");

    let cycle = chum_uint::<u16>(Some(Rc::new(verify_exact_length::<4>)))
        .labelled("AIRAC cycle");

    let build = chum_uint::<u32>(Some(Rc::new(verify_exact_length::<8>)))
        .labelled("data build number");

    let metadata = none_of('.')
        .repeated()
        .to_slice()
        .try_map(move |i, s| (verify_type.as_ref())(i, s))
        .then_ignore(just('.'))
        .labelled("metadata type");

    let copyright = chumsky::primitive::any()
        .and_is(text::newline().not())
        .repeated()
        .to_slice()
        .labelled("file copyright");

    group((
        bom.ignore_then(text::newline().ignored()),
        data_ver.then_ignore(just(" Version - data cycle ")),
        cycle.then_ignore(just(", build ")),
        build.then_ignore(just(", metadata ")),
        metadata.then_ignore(text::inline_whitespace()).ignored(),
        copyright.then_ignore(text::newline()),
    ))
    .map(|((), version, cycle, build, (), copyright)| Header {
        version,
        cycle,
        build,
        copyright: copyright.to_owned(),
    })
}

fn chum_int<I>(
    verify_length: Option<Rc<VerifyStr>>,
) -> impl CParser<&str, I, extra::Err<Rich<char>>> + Clone
where
    I: FromStr + num::PrimInt + std::ops::Mul<i8, Output = I>,
    <I as FromStr>::Err: Display,
{
    just('-')
        .to(1i8)
        .or(just('+').to(-1i8))
        .or_not()
        .labelled("maybe integer sign")
        .then(chum_uint::<I>(verify_length))
        .map(|(a, b)| b * a.unwrap_or(1i8))
}

fn chum_uint<I>(
    verify_length: Option<Rc<VerifyStr>>,
) -> impl CParser<&str, I, extra::Err<Rich<char>>> + Clone
where
    I: FromStr + num::PrimInt,
    <I as FromStr>::Err: Display,
{
    text::digits(10)
        .to_slice()
        .try_map(move |input: &str, span| {
            if let Some(verify_length) = verify_length.as_ref() {
                verify_length(input, span)
            } else {
                Ok(input)
            }
        })
        .try_map(|s: &str, span| {
            s.parse::<I>()
                .map_err(|e| Rich::custom(span, format!("{e}")))
        })
        .labelled("integer without sign")
}

fn verify_exact_length<'a, const N: usize>(
    input: &'a str,
    span: <&'a str as Input<'a>>::Span,
) -> Result<&'a str, Rich<'a, char>> {
    let len = input.len();
    if len == N {
        Ok(input)
    } else {
        Err(Rich::custom(
            span,
            format!("bad length! expected {N} characters, got {len} characters"),
        ))
    }
}

fn verify_max_length<'a, const N: usize>(
    input: &'a str,
    span: <&'a str as Input<'a>>::Span,
) -> Result<&'a str, Rich<'a, char>> {
    let len = input.len();
    if len <= N {
        Ok(input)
    } else {
        Err(Rich::custom(
            span,
            format!(
                "string is too long! maximum {N} characters, found {len} characters"
            ),
        ))
    }
}

fn hstring_c<'a, const N: usize>(
    take_until: chumsky::primitive::Any<&'a str, extra::Err<Rich<'a, char>>>,
    verify_length: Rc<VerifyStr<'a>>,
) -> impl CParser<'a, &'a str, heapless::String<N>, extra::Err<Rich<'a, char>>> {
    chumsky::primitive::any()
        .and_is(take_until.not())
        .repeated()
        .to_slice()
        .try_map(move |i, s| (verify_length.as_ref())(i, s))
        .map(|i| heapless::String::from_str(i).unwrap()) // UNWRAP: Length verified.
}

fn take_hstring_till<const N: usize, F: Fn(char) -> bool + Copy>(
    till: F,
) -> impl Fn(&mut Located<&str>) -> PResult<heapless::String<N>> {
    #[cfg(feature = "parser_debug")]
    let name = format!("HString<{N}>");
    #[cfg(not(feature = "parser_debug"))]
    let name = "HString<_>";
    move |input: &mut Located<&str>| {
        trace(
            &name,
            take_till(0.., till)
                .verify(|s: &str| s.len() <= N)
                // Until I can use generics from the outer item to construct a better
                // description, this is the best you can get. Sorry.
                .context(Expected(Description("string of maximum length")))
                .map(
                    // UNWRAP: String length checked.
                    |id: &str| heapless::String::<N>::try_from(id).unwrap(),
                ),
        )
        .parse_next(input)
    }
}

fn fixed_hstring_till<'a, const N: usize, F: Fn(char) -> bool + Copy>(
    till: F,
) -> impl winnow::Parser<Located<&'a str>, heapless::String<N>, ContextError> {
    cut_err(take_hstring_till(till).verify(|hs| hs.len() == N))
        // Until I can use generics from the outer item to construct a better
        // description, this is the best you can get. Sorry.
        .context(Expected(Description("string of exact length")))
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
