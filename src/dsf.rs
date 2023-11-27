use byteorder::{LittleEndian, ReadBytesExt};
use snafu::prelude::*;
use std::{
    collections::HashMap,
    io::{Read, Seek, SeekFrom},
    ops::Range,
};

#[derive(Snafu, Debug)]
pub enum DsfError {
    #[snafu(display("An I/O error occurred!"))]
    #[snafu(context(false))]
    IoError { source: std::io::Error },
    #[snafu(display("Internal error. Tried to access a bad offset within the file."))]
    BadOffset,
    #[snafu(display("The file is not valid DSF."))]
    InvalidDsf,
    #[snafu(display("The DSF format version in the file is not supported"))]
    UnsupportedVersion,
}

#[derive(Debug)]
pub struct DsfReader<R: Read + Seek> {
    reader: R,
}

impl<R: Read + Seek> DsfReader<R> {
    pub fn new(mut reader: R) -> Result<DsfReader<R>, DsfError> {
        reader.seek(SeekFrom::Start(0))?;
        let mut hdr = [0u8; 8];
        reader.read_exact(&mut hdr)?;
        let is_7z = if &hdr[0..6] == b"7z\xbc\xaf\x27\x1c" {
            reader.seek(SeekFrom::Start(0))?;
            true
        } else if &hdr == b"XPLNEDSF" {
            false
        } else {
            return Err(DsfError::InvalidDsf);
        };
        let dsf_ver = reader.read_i32::<LittleEndian>()?;
        if dsf_ver != 1 {
            return Err(DsfError::UnsupportedVersion);
        }
        todo!()
    }
}
