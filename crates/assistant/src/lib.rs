//! The assistant layer: the LLM seam, the tool set, and the turn driver.
//!
//! Everything below the [`seam::Model`] trait is deterministic logic over
//! the store, tested with scripted fakes; nothing in this crate's test
//! suite (or its dependents') ever talks to a model. The real Anthropic
//! client lives in [`client`] and is exercised only by `evals/` and by
//! actual use.
//!
//! The turn driver ([`turn::Turn`]) is sans-IO, like the store's sync
//! `Peer`: callers shuttle model turns and tool results through it, so the
//! CLI can run it over a locally-owned store while the server interleaves
//! it with a locked one — model calls never hold the store lock.

pub mod client;
pub mod context;
pub mod error;
pub mod exchange;
pub mod extract;
pub mod fetch;
pub mod seam;
pub mod tools;
pub mod turn;
pub mod views;

pub use error::AssistantError;
