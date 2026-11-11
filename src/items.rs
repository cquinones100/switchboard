//! What mission control's views list, and the sidebar's short lists under the
//! tabs: items, each with a title, a line of detail, a state that colors it,
//! a time, and what Enter does with it. Every source (the PR and ticket
//! boards, CI, the merge queues, the inbox, the calendar, Slack, the
//! sessions, a plugin) gives its items in this one shape, so the views draw
//! them all alike, as lib/sidebar/views.js's `{ title, detail, state, time }`
//! and a plugin's items were drawn alike.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::mux::TabId;

/// How an item is colored: something needing you, something under way, or
/// something to know about (a plugin's `attention`, `active`, `idle`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Attention,
    Active,
    #[default]
    Idle,
}

/// What Enter does with an item when its menu isn't asked for: go to a tab,
/// open a link (through the Chrome-profile chooser), or open a tab in a
/// directory.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    #[default]
    None,
    Tab(TabId),
    Url(String),
    Dir(PathBuf),
}

/// One row of a view.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Item {
    /// Stable across refreshes, so the cursor stays on it.
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub state: State,
    /// When it happened or is due, in milliseconds since the epoch.
    #[serde(default)]
    pub time: Option<i64>,
    #[serde(default)]
    pub target: Target,
    /// The heading it goes under (a board's column, a calendar's hour).
    #[serde(default)]
    pub group: Option<String>,
    /// Its cells, for a view drawn as a table (the work map, Resources).
    #[serde(default)]
    pub cells: Vec<String>,
    /// Lines shown under it only when it is the chosen one.
    #[serde(default)]
    pub more: Vec<String>,
    /// A body of several lines (a Slack message as Slack draws it).
    #[serde(default)]
    pub lines: Vec<String>,
    /// What its source knows of it, for its menu's choices: a PR's number
    /// and repository, a ticket's identifier, an event's Google ID.
    #[serde(default)]
    pub data: Value,
}

impl Item {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Item {
        Item {
            id: id.into(),
            title: title.into(),
            ..Item::default()
        }
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Item {
        self.detail = detail.into();
        self
    }

    pub fn state(mut self, state: State) -> Item {
        self.state = state;
        self
    }

    pub fn time(mut self, ms: i64) -> Item {
        self.time = Some(ms);
        self
    }

    pub fn target(mut self, target: Target) -> Item {
        self.target = target;
        self
    }

    pub fn group(mut self, group: impl Into<String>) -> Item {
        self.group = Some(group.into());
        self
    }

    pub fn data(mut self, data: Value) -> Item {
        self.data = data;
        self
    }
}

/// A table's column: its heading and how much room it takes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub heading: String,
    /// Columns it takes at most; the last column takes what is left.
    pub width: u16,
    /// Numbers line up on the right.
    #[serde(default)]
    pub right: bool,
}

/// What a source gives a view: its items, or why it has none.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ViewData {
    pub items: Vec<Item>,
    /// What to do to set it up, when what it reads from isn't (no token, a
    /// connector missing), drawn whole in place of the items, as
    /// lib/sidebar/setup-help.js and connector.js's words were.
    #[serde(default)]
    pub setup: Option<String>,
    /// Why the last look failed, shown above the items it kept.
    #[serde(default)]
    pub error: Option<String>,
    /// What to say when it has no items, in place of a bare "nothing" (the
    /// Landing view saying nothing is landing).
    #[serde(default)]
    pub empty: Option<String>,
    /// For a table: its columns, and a totals row under it.
    #[serde(default)]
    pub columns: Vec<Column>,
    #[serde(default)]
    pub footer: Vec<String>,
    /// When it was last looked up, in milliseconds since the epoch.
    #[serde(default)]
    pub at: Option<i64>,
}
