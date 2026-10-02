//! Fennec core: everything that does not touch GTK.

// Byte-span lists (low-confidence words) often hold a single range on purpose.
#![allow(clippy::single_range_in_vec_init)]

pub mod audio;
pub mod commands;
pub mod config;
pub mod engine;
pub mod eval;
pub mod export;
pub mod ingest;
pub mod live;
pub mod models;
pub mod paragraphs;
pub mod store;
pub mod template;
pub mod text;
pub mod transcript;
pub mod ui;
pub mod utterance;
pub mod vad;
pub mod worker;
