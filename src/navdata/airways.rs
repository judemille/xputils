//! Parser and data structures for the X-Plane airways file.

use std::sync::Arc;

use crate::navdata::{Header, Waypoint};

#[derive(Debug)]
pub struct Airways {
    header: Header,
    entries: Vec<Arc<AwySegment>>
}

#[derive(Debug)]
pub struct Airway {
    name: String,
    segments: Vec<Arc<AwySegment>>
}

#[derive(Debug)]
pub struct AwySegment {
    base: u16,
    top: u16,
    point_1: Waypoint,
    point_2: Waypoint,
    is_high: bool,
    name: String
}

#[derive(Debug, Copy, Clone)]
pub enum AwyDirection {
    Bidirectional,
    Forward,
    Backward
}
