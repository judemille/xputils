#![warn(clippy::all, clippy::pedantic)]

#[cfg(feature = "dsf")]
pub mod dsf;

#[cfg(feature = "navdata")]
pub mod navdata;

#[cfg(test)]
mod tests {

}
