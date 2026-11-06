//! What the outside world says about each tab, gathered in the background and
//! read by the sidebar's model to place the tab (Build failing, In merge
//! queue, Landing, Merged): its pull request's state, merge-queue place and
//! checks, as lib/sidebar/prs.js kept them per tab, and its place in its
//! repository's landing queue, as lib/sidebar/landing.js did. The sources
//! (src/sources) write these; the model and views only read them, so nothing
//! on the drawing path waits for GitHub.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::mux::TabId;

/// A pull request's state, as GitHub names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

/// Where a pull request stands on its way in: queued at a place, or merged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Merge {
    Queued { position: u32 },
    Merged,
}

/// A pull request's checks counted by outcome, in gh pr checks' buckets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checks {
    pub pass: u32,
    pub fail: u32,
    pub pending: u32,
}

/// What GitHub says of a tab's pull request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrFacts {
    pub number: u64,
    /// `owner/name`.
    pub repo: String,
    pub url: String,
    pub state: PrState,
    pub merge: Option<Merge>,
    /// Left out once it has merged.
    pub checks: Option<Checks>,
    /// APPROVED, CHANGES_REQUESTED, REVIEW_REQUIRED, or none.
    pub review_decision: Option<String>,
    pub additions: u64,
    pub deletions: u64,
}

/// A tab's place in its repository's landing queue: the one landing holds
/// the lock (line 1), the rest wait in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Landing {
    pub holding: bool,
    pub line: u32,
}

/// Everything known of one tab from outside.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabFacts {
    pub pr: Option<PrFacts>,
    pub landing: Option<Landing>,
}

/// Every tab's facts, held by the server.
pub type Facts = HashMap<TabId, TabFacts>;
