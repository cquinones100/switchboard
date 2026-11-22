//! A workspace's emoji on the server: what was loaded of each name, and one
//! load off the task for the names a draw wants that aren't held, so the
//! Slack view asks `Server::slack_emoji` as it draws and gets each picture
//! once it is there, every client drawn again as they arrive. A name held is
//! loaded again once `KEEP` runs out; a load that failed is tried again
//! after `RETRY_MS`, not on every draw.

use std::collections::{HashMap, HashSet};

use super::{Custom, Store};
use crate::mux::server::{Event, Server};
use crate::util;

/// How long after a failed load a name is asked for again.
const RETRY_MS: i64 = 15 * 60 * 1000;

#[derive(Default)]
pub struct SlackEmojis {
    /// Each name loaded, and when it is due to be loaded again.
    held: HashMap<String, (Custom, i64)>,
    /// When a load of a name not held failed, for its retry.
    failed: HashMap<String, i64>,
    /// The names being loaded now.
    loading: HashSet<String>,
}

#[derive(Debug)]
pub enum SlackEmojisEvent {
    /// What was found of the names asked, and the names asked.
    Loaded(Vec<(String, Custom)>, Vec<String>),
    Failed(Vec<String>),
}

impl Server {
    /// What each of `names` is, of those loaded, the rest loaded off the
    /// task; nothing while Settings' `slack_emoji` is off.
    pub fn slack_emoji(
        &mut self,
        names: impl IntoIterator<Item = String>,
    ) -> HashMap<String, Custom> {
        let mut out = HashMap::new();
        if !super::on() {
            return out;
        }
        let now = util::now_ms();
        let mut wanted = Vec::new();
        for name in names {
            if out.contains_key(&name) || wanted.contains(&name) {
                continue;
            }
            let due = match self.slack_emojis.held.get(&name) {
                Some((custom, due)) => {
                    out.insert(name.clone(), custom.clone());
                    *due <= now
                }
                None => self
                    .slack_emojis
                    .failed
                    .get(&name)
                    .is_none_or(|at| now - at >= RETRY_MS),
            };
            if due && !self.slack_emojis.loading.contains(&name) {
                wanted.push(name);
            }
        }
        if wanted.is_empty() || tokio::runtime::Handle::try_current().is_err() {
            return out;
        }
        self.slack_emojis.loading.extend(wanted.iter().cloned());
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let event = match super::load(&Store::here(), &wanted).await {
                Ok(found) => SlackEmojisEvent::Loaded(found, wanted),
                Err(_) => SlackEmojisEvent::Failed(wanted),
            };
            let _ = tx.send(Event::SlackEmojis(event));
        });
        out
    }

    pub(crate) fn slack_emojis_event(&mut self, event: SlackEmojisEvent) {
        let now = util::now_ms();
        let (found, asked) = match event {
            SlackEmojisEvent::Loaded(found, asked) => (found, asked),
            SlackEmojisEvent::Failed(asked) => (Vec::new(), asked),
        };
        for name in &asked {
            self.slack_emojis.loading.remove(name);
        }
        let mut changed = false;
        let found_names: HashSet<String> = found.iter().map(|(n, _)| n.clone()).collect();
        for (name, custom) in found {
            self.slack_emojis.failed.remove(&name);
            let due = now + super::KEEP.as_millis() as i64;
            self.slack_emojis.held.insert(name, (custom, due));
            changed = true;
        }
        for name in asked.into_iter().filter(|n| !found_names.contains(n)) {
            // One held (its cache emptied underneath) waits as one never
            // loaded does.
            if let Some((_, due)) = self.slack_emojis.held.get_mut(&name) {
                *due = now + RETRY_MS;
            }
            self.slack_emojis.failed.insert(name, now);
        }
        if changed {
            self.mission_changed();
            for c in self.clients.values_mut() {
                c.dirty = true;
            }
        }
    }
}
