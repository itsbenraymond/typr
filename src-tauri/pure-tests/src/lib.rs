//! Mirrors the main crate's module paths so `crate::vocab` resolves the same.
#[path = "../../src/vocab.rs"]
pub mod vocab;
#[path = "../../src/learn.rs"]
pub mod learn;
#[path = "../../src/command.rs"]
pub mod command;
#[path = "../../src/listfmt.rs"]
pub mod listfmt;
