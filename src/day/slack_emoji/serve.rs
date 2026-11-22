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
    /// The workspace what is held came from: one signed in to since lets
    /// all of it go.
    team: String,
    /// When the sign-in was last read for its workspace.
    team_read_at: i64,
}

/// How often the sign-in's workspace is read again, a small file.
const TEAM_EVERY_MS: i64 = 5_000;

#[derive(Debug)]
pub enum SlackEmojisEvent {
    /// The workspace looked in, what was found of the names asked, and the
    /// names asked.
    Loaded(String, Vec<(String, Custom)>, Vec<String>),
    Failed(Vec<String>),
}

impl Server {
    /// What each of `names` is, of those loaded (a name whose load failed
    /// as `Unknown`), the rest loaded off the task; nothing while Settings'
    /// `slack_emoji` is off.
    pub fn slack_emoji(
        &mut self,
        names: impl IntoIterator<Item = String>,
    ) -> HashMap<String, Custom> {
        let mut out = HashMap::new();
        if !super::on() {
            return out;
        }
        let now = util::now_ms();
        if now - self.slack_emojis.team_read_at >= TEAM_EVERY_MS {
            self.slack_emojis.team_read_at = now;
            self.slack_emojis_from(super::signed_in_team().unwrap_or_default());
        }
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
                None => match self.slack_emojis.failed.get(&name) {
                    // A name that couldn't be looked up (no sign-in, the
                    // scope missing, Slack out of reach) is drawn as its code
                    // until a retry finds it, never as a cut placeholder.
                    Some(at) => {
                        out.insert(name.clone(), Custom::Unknown);
                        now - at >= RETRY_MS
                    }
                    None => true,
                },
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
            let team = super::signed_in_team();
            let found = match &team {
                Some(team) => super::load(&Store::of(team), &wanted).await,
                None => Err("no Slack sign-in".to_string()),
            };
            let event = match found {
                Ok(found) => SlackEmojisEvent::Loaded(team.unwrap_or_default(), found, wanted),
                Err(_) => SlackEmojisEvent::Failed(wanted),
            };
            let _ = tx.send(Event::SlackEmojis(event));
        });
        out
    }

    /// Lets go of what is held when it came from another workspace than
    /// `team`, the one signed in to now (empty with none).
    fn slack_emojis_from(&mut self, team: String) {
        if team != self.slack_emojis.team {
            self.slack_emojis.held.clear();
            self.slack_emojis.failed.clear();
            self.slack_emojis.team = team;
        }
    }

    pub(crate) fn slack_emojis_event(&mut self, event: SlackEmojisEvent) {
        let now = util::now_ms();
        let (found, asked) = match event {
            SlackEmojisEvent::Loaded(team, found, asked) => {
                self.slack_emojis_from(team);
                (found, asked)
            }
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
        // A failed name is now drawn as its code, so the board changes too.
        for name in asked.into_iter().filter(|n| !found_names.contains(n)) {
            changed = true;
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
