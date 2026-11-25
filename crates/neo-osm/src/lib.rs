//! # neo-osm — OSM domain model
//!
//! This layer expresses **OSM semantics** only; it does not care about storage layout, render format, or UI:
//!
//! * [`Tags`]: a tag set made of interned symbol pairs;
//! * [`Node`] / [`Way`] / [`Relation`]: DTOs used by IO, API, clipboard, and command parameters;
//! * [`ChangeState`] / [`ChangeAction`]: local edit state and upload action semantics.
//!
//! Physical storage (SoA, handles, indexes) lives in `neo-store`; the two are deliberately separated, see
//! `docs/03-osm-data-model-and-store.md` and ADR-0004.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod change;
pub mod element;
pub mod tags;

pub use change::{ChangeAction, ChangeState};
pub use element::{Element, Node, Relation, RelationMember, Way};
pub use tags::Tags;
