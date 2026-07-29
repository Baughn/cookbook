//! Domain core: types and pure math for readiness, coverage, lead time,
//! and rotation. No IO, no CRDT machinery; the clock is always a parameter.

pub mod coverage;
pub mod readiness;
pub mod rotation;
pub mod types;
