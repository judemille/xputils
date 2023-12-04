#![cfg_attr(feature = "RUSTC_IS_NIGHTLY", feature(generic_const_exprs))]

#[cfg(feature = "dsf")]
pub mod dsf;

#[cfg(feature = "navdata")]
pub mod navdata;

#[cfg(test)]
mod tests {}
