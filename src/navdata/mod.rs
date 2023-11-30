pub mod airways;
pub mod fix;
pub mod nav;

use snafu::{prelude::*, Backtrace};
use std::{
    fs::File,
    io::{BufRead, BufReader, Error as IoError, Lines, Read},
    ops::Deref,
    path::Path,
    sync::Arc,
};
use winnow::{
    ascii::{digit1, space0, space1},
    combinator::{fail, preceded, rest, success},
    dispatch,
    error::ContextError,
    prelude::*,
    stream::AsChar,
    token::{take, take_till1, take_until1},
};

use crate::navdata::{fix::{Fixes, Fix}, nav::{Navaids, Navaid}};

pub struct NavigationalData {
    fixes: Fixes,
    navaids: Navaids,
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
            let main_fixes_entries = fixes.entries_mut();
            for user_fix in user_fixes.entries {
                // Essentially, check if there is a fix in the same area, with the same ident.
                let matching_main_fix_pos = main_fixes_entries.iter().position(|fix| {
                    fix.ident == user_fix.ident
                        && fix.icao_region == user_fix.icao_region
                        && fix.terminal_area == user_fix.terminal_area
                });
                if let Some(pos) = matching_main_fix_pos {
                    main_fixes_entries[pos] = user_fix;
                } else {
                    main_fixes_entries.push(user_fix);
                }
            }
        }
        let nav_file = BufReader::new(File::open(folder.join("earth_nav.dat"))?);
        let mut navaids = nav::parse_file_buffered(nav_file)?;
        let user_nav = folder.join("user_nav.dat");
        if user_nav.exists() {
            let user_nav = BufReader::new(File::open(user_nav)?);
            let user_nav = nav::parse_file_buffered(user_nav)?;
            let main_nav_entries = navaids.entries_mut();
            for user_navaid in user_nav.entries {
                // Essentially, check if there is a matching navaid of the same type, in the same place, with the same ident.
                let matching_main_navaid_pos =
                    main_nav_entries.iter().position(|navaid| {
                        navaid.ident == user_navaid.ident
                            && navaid.icao_region_code == user_navaid.icao_region_code
                            && std::mem::discriminant(&navaid.type_data)
                                == std::mem::discriminant(&user_navaid.type_data)
                    });
                if let Some(pos) = matching_main_navaid_pos {
                    main_nav_entries[pos] = user_navaid;
                } else {
                    main_nav_entries.push(user_navaid);
                }
            }
        }
        Ok(Self { fixes, navaids })
    }

    #[must_use]
    pub fn fixes(&self) -> &Fixes {
        &self.fixes
    }

    #[must_use]
    pub fn navaids(&self) -> &Navaids {
        &self.navaids
    }
}

#[derive(Debug, Clone)]
pub enum Waypoint {
    Fix(Arc<Fix>),
    Navaid(Arc<Navaid>)
}

#[derive(Debug, Copy, Clone)]
pub enum DataVersion {
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
    let verify_type = Arc::new(verify_type); // Gets rid of stupid lifetime errors.
    move |input: &mut &str| -> PResult<Header> {
        let version = dispatch! {take(4usize);
            "1101" => success(DataVersion::XP1101),
            "1140" => success(DataVersion::XP1140),
            "1150" => success(DataVersion::XP1150),
            "1200" => success(DataVersion::XP1200),
            _ => fail
        }
        .parse_next(input)?;
        " Version - data cycle ".parse_next(input)?;
        let cycle: u16 = take(4u8)
            .and_then(digit1)
            .try_map(|s: &str| s.parse())
            .parse_next(input)?;

        ", build ".parse_next(input)?;
        let build: u32 = take(8u8)
            .and_then(digit1)
            .try_map(|s: &str| s.parse())
            .parse_next(input)?;

        ", metadata ".parse_next(input)?;
        take_until1(".")
            .verify(verify_type.deref())
            .parse_next(input)?;
        '.'.parse_next(input)?;
        let copyright = preceded(space0, rest).parse_next(input)?.to_string();
        Ok(Header {
            version,
            cycle,
            build,
            copyright,
        })
    }
}

fn parse_fixed_str<const N: usize>(input: &mut &str) -> PResult<heapless::String<N>> {
    preceded(space1, take_till1(|c: char| c.is_space()))
        .try_map(|id: &str| {
            heapless::String::<N>::try_from(id).map_err(|()| StringTooLarge)
        })
        .parse_next(input)
}
