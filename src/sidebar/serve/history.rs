//! The notifications' history on the server (`sidebar::history`): read once
//! as it starts, every source's list recorded as `set_notices` takes it, a
//! notification dismissed or read marked so, what one led to recorded where
//! it is known, and the changed days written on the tick, one write at a
//! time, all off the server's task but the recording itself.

use std::collections::BTreeMap;

use super::super::SidebarEvent;
use super::super::history::{self, Entry, Sidebar};
use crate::mux::server::{Event, Server};
use crate::{settings, util};

impl Server {
    /// Whether notifications are kept: the history's setting, under the
    /// sidebar's notifications.
    pub(crate) fn notice_history_on() -> bool {
        settings::is_on("sidebar_notifications") && settings::is_on("notification_history")
    }

    /// Reads the kept days off the server's task, as the server starts.
    pub(crate) fn notice_history_load(&mut self) {
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let read = history::read(&history::dir());
            let _ = tx.send(Event::Sidebar(SidebarEvent::HistoryRead(read)));
        });
    }

    /// The kept days read, then every source that has given its list since
    /// the server started recorded as its list is now: one the sidebar
    /// showed when the server last stopped and no longer does is older,
    /// one a source gives again shown, one new since it started an entry.
    /// Nothing is recorded before, so a source answering first never
    /// stands in for what was kept of the same notification, and a source
    /// yet to answer is recorded when it does.
    pub(crate) fn notice_history_read(&mut self, read: BTreeMap<String, Vec<Entry>>) {
        let h = &mut self.sidebar.history;
        h.take_loaded(read);
        let sources = h.answered.clone();
        for source in sources {
            self.notice_history_record(&source);
        }
        self.mission_changed();
    }

    /// A source's notifications as they are now, recorded.
    pub(crate) fn notice_history_record(&mut self, source: &str) {
        // A source counts as having answered once it lists something: an
        // empty list may be one it gives before it has read anything (the
        // calendar before its feed answers).
        if self.notices.contains_key(source) {
            self.sidebar.history.answered.insert(source.to_string());
        }
        let answered = self.sidebar.history.answered.contains(source);
        if !Self::notice_history_on() || !self.sidebar.history.loaded || !answered {
            return;
        }
        let items = self.notices.get(source).cloned().unwrap_or_default();
        let tz = jiff::tz::TimeZone::system();
        if self
            .sidebar
            .history
            .record(source, &items, util::now_ms(), &tz)
        {
            self.mission_changed();
        }
    }

    /// A notification put out of the sidebar by you: dismissed, or read.
    /// With `late`, read over dismissed too (`History::mark`).
    pub(crate) fn notice_history_mark(&mut self, source: &str, id: &str, how: Sidebar, late: bool) {
        if Self::notice_history_on() && self.sidebar.history.mark(source, id, how, late) {
            self.mission_changed();
        }
    }

    /// A source's notifications put away by you, as dismissed: the one
    /// with this id, or every one it shows.
    pub(crate) fn notices_dismissed(&mut self, source: &str, id: Option<&str>) {
        let ids: Vec<String> = match id {
            Some(id) => vec![id.to_string()],
            None => self
                .notices
                .get(source)
                .map(|items| items.iter().map(|i| i.id.clone()).collect())
                .unwrap_or_default(),
        };
        for id in ids {
            self.notice_history_mark(source, &id, Sidebar::Dismissed, false);
        }
    }

    /// A source's notifications kept in the history as dismissed without the
    /// sidebar ever showing them (what a setting keeps quiet), and none of
    /// the source's left in the sidebar.
    pub(crate) fn notices_put_away(&mut self, source: &str, items: Vec<crate::items::Item>) {
        let ids: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
        self.set_notices(source, items);
        for id in ids {
            self.notice_history_mark(source, &id, Sidebar::Dismissed, false);
        }
        self.set_notices(source, Vec::new());
    }

    /// What a notification led to, in a few words ("you went to tab 5"):
    /// the occurrence that came at `occurrence` when given, else its
    /// latest.
    pub(crate) fn led_to(&mut self, source: &str, id: &str, occurrence: Option<i64>, words: &str) {
        if Self::notice_history_on()
            && self
                .sidebar
                .history
                .led_to(source, id, occurrence, words, util::now_ms())
        {
            self.mission_changed();
        }
    }

    /// On the tick: the days past the kept ones let go, and the changed
    /// days written, once the files have been read and no write is under
    /// way.
    pub(crate) fn notice_history_tick(&mut self) {
        // Off, nothing more is kept.
        let h = &mut self.sidebar.history;
        if !Self::notice_history_on() || !h.loaded || self.sidebar.history_writing {
            return;
        }
        let today = history::day_of(util::now_ms(), &jiff::tz::TimeZone::system());
        let gone = h.prune(&today);
        let days = h.take_changed();
        if days.is_empty() && gone.is_empty() {
            return;
        }
        self.sidebar.history_writing = true;
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            history::write(&history::dir(), &days, &gone);
            let _ = tx.send(Event::Sidebar(SidebarEvent::HistoryWritten));
        });
    }
}
