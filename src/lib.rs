#![cfg_attr(RUSTC_IS_NIGHTLY, feature(generic_const_exprs))]

// SPDX-FileCopyrightText: 2024 Julia DeMille <me@jdemille.com>
//
// SPDX-License-Identifier: Parity-7.0.0

#[cfg(feature = "dsf")]
pub mod dsf;

#[cfg(feature = "navdata")]
pub mod navdata;

#[cfg(test)]
mod tests {}
