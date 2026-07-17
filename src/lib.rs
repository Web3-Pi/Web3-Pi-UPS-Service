//! Library surface of the w3p-ups crate.
//!
//! Exposes the wire-protocol implementation so sibling binaries (currently
//! `ups-live`, the desktop live dashboard) can reuse the exact same framer,
//! checksum, and payload catalogue as the agent instead of duplicating the
//! protocol. The agent binary (`src/main.rs`) keeps its own private module
//! tree and is unaffected.

pub mod proto;
