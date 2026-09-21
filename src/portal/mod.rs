//! Portals: the code forges GitHoot can watch.
//!
//! Today there is one, GitHub. This module exists so that is a fact about the contents of the
//! directory and not about the shape of the program: the core (`state`, `page`, `scheduler`) speaks
//! the vocabulary in `types` and nothing else.

pub mod github;
pub mod statuspage;
pub mod types;
