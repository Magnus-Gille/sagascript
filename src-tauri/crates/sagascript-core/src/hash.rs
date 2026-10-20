//! SHA-256 as Sagascript writes it down: 64 lowercase hexadecimal digits (#373).
//!
//! Source hashes of meetings and diarization caches, the revisions of reviews
//! and reprocessing proposals, context hashes, the staging names of exports and
//! the integrity of every download are spelled this one way. Before this module
//! each place formatted a digest itself, three different ways to the same text.
//!
//! [`sha256_hex`] is for data in memory, [`StreamingSha256`] for data that
//! arrives in pieces (a file read in chunks, a download). What a caller does
//! around the hashing (cancelling, capping the size, naming its own errors) is
//! its own.

use sha2::{Digest, Sha256};

/// The SHA-256 of `bytes` as 64 lowercase hexadecimal digits.
pub fn sha256_hex(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}

/// SHA-256 over data that arrives in pieces; [`finish_hex`](Self::finish_hex)
/// is [`sha256_hex`] of everything fed in, whatever the pieces were.
#[derive(Default)]
pub struct StreamingSha256(Sha256);

impl StreamingSha256 {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, bytes: impl AsRef<[u8]>) {
        self.0.update(bytes.as_ref());
    }

    /// The digest as 64 lowercase hexadecimal digits.
    pub fn finish_hex(self) -> String {
        format!("{:x}", self.0.finalize())
    }
}

#[cfg(test)]
#[path = "hash_tests.rs"]
mod tests;
