pub mod config;
pub mod envelope;
pub mod error;
pub mod imap;
pub mod smtp;
pub mod supervisor;
pub mod wire;

pub const CHUNK: usize = 1024 * 1024;
pub const HEADER_LIMIT: usize = 64 * 1024;
pub const BATCH: u32 = 100;
pub const FAILURE_LIMIT: u8 = 5;
