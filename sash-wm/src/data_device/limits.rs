// SPDX-License-Identifier: Apache-2.0

//! The data device limits (owner, M10): what a source may advertise, and how often an
//! offer may be read. Pure, so tests can use fake time.

use std::{collections::VecDeque, time::Duration};

/// MIME types per source.
pub const MAX_MIME_TYPES: usize = 32;
/// Bytes of one MIME type name.
pub const MAX_MIME_LEN: usize = 256;
/// Bytes of all MIME type names of one source.
pub const MAX_MIME_BYTES: usize = 4096;
/// Live offers per client; a client holding this many gets no new ones.
pub const MAX_OFFERS_PER_CLIENT: usize = 16;
/// Receive requests per offer within RECEIVE_WINDOW (sliding).
pub const MAX_RECEIVES: usize = 16;
pub const RECEIVE_WINDOW: Duration = Duration::from_secs(1);

/// Why a MIME type was not added.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MimeRefusal {
    Count,
    Length,
    Bytes,
}

/// The MIME types a source advertises, within the limits. Duplicates are kept once.
#[derive(Debug, Default)]
pub struct MimeList {
    names: Vec<String>,
    bytes: usize,
}

impl MimeList {
    pub fn add(&mut self, name: String) -> Result<(), MimeRefusal> {
        if self.names.contains(&name) {
            return Ok(());
        }
        if name.len() > MAX_MIME_LEN {
            return Err(MimeRefusal::Length);
        }
        if self.names.len() >= MAX_MIME_TYPES {
            return Err(MimeRefusal::Count);
        }
        if self.bytes + name.len() > MAX_MIME_BYTES {
            return Err(MimeRefusal::Bytes);
        }
        self.bytes += name.len();
        self.names.push(name);
        Ok(())
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    pub fn contains(&self, name: &str) -> bool {
        self.names.iter().any(|n| n == name)
    }
}

/// Receive requests of one offer in the last RECEIVE_WINDOW.
#[derive(Debug, Default)]
pub struct ReceiveRate {
    recent: VecDeque<Duration>,
}

impl ReceiveRate {
    /// A receive request at `now` (any monotonic clock): allowed if fewer than
    /// MAX_RECEIVES were allowed in the RECEIVE_WINDOW before it.
    pub fn allow(&mut self, now: Duration) -> bool {
        while self
            .recent
            .front()
            .is_some_and(|t| now.saturating_sub(*t) >= RECEIVE_WINDOW)
        {
            self.recent.pop_front();
        }
        if self.recent.len() >= MAX_RECEIVES {
            return false;
        }
        self.recent.push_back(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_types_are_limited_by_count_length_and_total_bytes() {
        let mut list = MimeList::default();
        for i in 0..MAX_MIME_TYPES {
            assert_eq!(list.add(format!("type/{i}")), Ok(()));
        }
        assert_eq!(list.add("type/one-more".into()), Err(MimeRefusal::Count));
        assert_eq!(
            list.add("type/0".into()),
            Ok(()),
            "a duplicate is not counted"
        );
        assert_eq!(list.names().len(), MAX_MIME_TYPES);

        let mut list = MimeList::default();
        let long = format!("text/{}", "x".repeat(MAX_MIME_LEN));
        assert_eq!(list.add(long), Err(MimeRefusal::Length));
        let at_limit = "x".repeat(MAX_MIME_LEN);
        assert_eq!(list.add(at_limit), Ok(()), "exactly the limit is allowed");

        let mut list = MimeList::default();
        // 16 names of 256 bytes are 4096 bytes: the 17th does not fit.
        for i in 0..16 {
            assert_eq!(list.add(format!("{i:0>256}")), Ok(()));
        }
        assert_eq!(list.add("text/plain".into()), Err(MimeRefusal::Bytes));
        assert!(list.contains(&format!("{:0>256}", 3)));
        assert!(!list.contains("text/plain"));
    }

    /// Owner's test: 40 pastes of one selection over 10 seconds all succeed; 100
    /// requests in one second get end of file beyond the 16th.
    #[test]
    fn receives_are_limited_per_second_sliding() {
        let mut rate = ReceiveRate::default();
        for i in 0..40u64 {
            assert!(rate.allow(Duration::from_millis(i * 250)), "paste {i}");
        }
        let mut rate = ReceiveRate::default();
        let allowed: Vec<bool> = (0..100u64)
            .map(|i| rate.allow(Duration::from_millis(i * 10)))
            .collect();
        assert!(allowed[..16].iter().all(|a| *a));
        assert!(
            allowed[16..].iter().all(|a| !*a),
            "beyond the 16th in one second"
        );
        // Sliding: one second after the first, one more is allowed.
        assert!(rate.allow(Duration::from_millis(1000)));
        assert!(!rate.allow(Duration::from_millis(1001)));
    }
}
