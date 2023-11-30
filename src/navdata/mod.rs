pub mod fix;
pub mod nav;

use snafu::prelude::*;
use std::{
    io::{BufRead, Error as IoError, Lines, Read},
    ops::Deref,
    sync::Arc,
};
use winnow::{
    ascii::{digit1, space0, space1},
    combinator::{fail, preceded, rest, success},
    dispatch,
    error::ContextError,
    prelude::*,
    token::{take, take_until1, take_till1}, stream::AsChar,
};

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
    Io { source: IoError },
    #[snafu(display("Error occurred when parsing `{stage}`: \n\n{rendered}"))]
    Parse { rendered: String, stage: String },
    #[snafu(display("The byte order marker was an unexpected value: {bom}"))]
    BadBOM { bom: String },
    #[snafu(display("The last line of this file was unexpected:\n{last_line}"))]
    BadLastLine { last_line: String },
    #[snafu(display("A line was expected, but the file had no more."))]
    MissingLine,
    #[snafu(display(
        "The data version {version:?} is not supported by the parser for this format."
    ))]
    UnsupportedVersion { version: DataVersion },
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
