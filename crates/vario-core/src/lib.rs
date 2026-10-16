//! Core library of Vario. The CLI, the hooks, the MCP server and the interface are
//! thin front ends over it.

pub mod error;
pub mod files;
pub mod parts;
pub mod paths;

pub use error::{Error, Result};
