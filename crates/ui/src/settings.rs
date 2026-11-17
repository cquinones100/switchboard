//! The settings the drawing follows, asked of the program that draws:
//! whether UX 2.0 is on, the theme, the scroll bars, and what a box's look
//! reads (`Look::read`). This crate knows no settings file; the program
//! says where its settings are (`install`), once, as it starts:
//! `switchboard` gives its own (`~/.config/switchboard/settings.json` over
//! `assets/settings.json`), an app alone in a terminal whatever it reads.
//! Until one does, each key answers its default (`DEFAULTS`, which a test
//! in `switchboard` keeps the same as `assets/settings.json`'s), so a unit
//! test draws as the defaults do.

use std::sync::OnceLock;

use serde_json::Value;

/// How a program's settings are read.
pub struct Source {
    /// One option's value as last read, for what asks many times a second.
    pub cached: fn(&str) -> Option<Value>,
    /// Each of these options' values read now, in their order.
    pub fresh: fn(&[&str]) -> Vec<Option<Value>>,
}

static SOURCE: OnceLock<Source> = OnceLock::new();

/// Says where the program's settings are. The first one stays.
pub fn install(source: Source) {
    let _ = SOURCE.set(source);
}

/// What a default is, written as a constant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Default {
    On,
    Off,
    Choice(&'static str),
}

impl Default {
    pub fn value(self) -> Value {
        match self {
            Default::On => Value::Bool(true),
            Default::Off => Value::Bool(false),
            Default::Choice(c) => Value::String(c.to_string()),
        }
    }
}

/// Every setting this crate reads, with its default.
pub const DEFAULTS: &[(&str, Default)] = &[
    ("ux2", Default::Off),
    ("scroll_bars", Default::On),
    ("theme", Default::Choice("dark")),
    ("terminal_colors", Default::Off),
    ("menu_mouse", Default::On),
    ("hover_highlight", Default::On),
    ("easy_keys", Default::Off),
    ("tab_focus", Default::On),
];

fn default_of(key: &str) -> Option<Value> {
    DEFAULTS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, d)| d.value())
}

/// One option's value as last read.
pub fn cached(key: &str) -> Option<Value> {
    match SOURCE.get() {
        Some(source) => (source.cached)(key),
        None => default_of(key),
    }
}

/// Whether an on/off option is on, as last read. One that isn't listed
/// counts as on.
pub fn is_on(key: &str) -> bool {
    cached(key) != Some(Value::Bool(false))
}

/// Each of these options' values read now, in their order.
pub fn fresh(keys: &[&str]) -> Vec<Option<Value>> {
    match SOURCE.get() {
        Some(source) => (source.fresh)(keys),
        None => keys.iter().map(|k| default_of(k)).collect(),
    }
}
