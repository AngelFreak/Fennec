//! Fennec core: everything that does not touch GTK.

// Byte-span lists (low-confidence words) often hold a single range on purpose.
#![allow(clippy::single_range_in_vec_init)]

pub mod audio;
pub mod engine;
pub mod eval;
pub mod export;
pub mod ingest;
pub mod paragraphs;
pub mod store;
pub mod template;
pub mod text;
pub mod vad;
