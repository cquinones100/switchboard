//! People's photos on the server: what was loaded of each, and a load off
//! the task for one not yet asked for, so whatever draws an avatar asks
//! `Server::photo` on every draw and gets the photo once it is there,
//! every client drawn again as one arrives. A photo held is loaded again
//! once it is due, when its cache's `KEEP` runs out, `RETRY_MS`
//! after an old one kept because fetching it again failed, the one held
//! shown meanwhile; a load that failed with nothing kept is tried again
//! after `RETRY_MS`, not on every draw.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use image::RgbaImage;

use super::{Loaded, Store, Who};
use crate::mux::server::{Event, Server};
use crate::{settings, util};

/// How long after a failed fetch the photo is asked for again.
const RETRY_MS: i64 = 15 * 60 * 1000;

#[derive(Default)]
pub struct Photos {
    /// Each person loaded: their photo (`None` for someone without one),
    /// and when it is due to be loaded again.
    held: HashMap<Who, (Option<Arc<RgbaImage>>, i64)>,
    /// When a load that found nothing failed, for its retry.
    failed: HashMap<Who, i64>,
    /// Those being loaded now.
    loading: HashSet<Who>,
}

#[derive(Debug)]
pub enum PhotosEvent {
    Loaded(Who, Loaded),
    Failed(Who),
}

impl Server {
    /// Someone's photo for an avatar, `None` until it has loaded, for
    /// someone without one, and while the `photos` setting is off.
    pub fn photo(&mut self, who: &Who) -> Option<Arc<RgbaImage>> {
        if !settings::is_on("photos") {
            return None;
        }
        let now = util::now_ms();
        let (shown, due) = match self.photos.held.get(who) {
            Some((photo, due)) => (photo.clone(), *due <= now),
            None => {
                let waiting = self
                    .photos
                    .failed
                    .get(who)
                    .is_some_and(|at| now - at < RETRY_MS);
                (None, !waiting)
            }
        };
        if due
            && !self.photos.loading.contains(who)
            && tokio::runtime::Handle::try_current().is_ok()
        {
            self.photos.loading.insert(who.clone());
            let tx = self.tx.clone();
            let who = who.clone();
            tokio::spawn(async move {
                let event = match super::load(&Store::here(), &who).await {
                    Ok(loaded) => PhotosEvent::Loaded(who, loaded),
                    Err(_) => PhotosEvent::Failed(who),
                };
                let _ = tx.send(Event::Photos(event));
            });
        }
        shown
    }

    pub(crate) fn photos_event(&mut self, event: PhotosEvent) {
        let now = util::now_ms();
        match event {
            PhotosEvent::Loaded(who, loaded) => {
                self.photos.loading.remove(&who);
                self.photos.failed.remove(&who);
                let due = now
                    + if loaded.fresh_for.is_zero() {
                        RETRY_MS
                    } else {
                        loaded.fresh_for.as_millis() as i64
                    };
                let photo = loaded.photo.map(Arc::new);
                let changed = photo.is_some()
                    || self
                        .photos
                        .held
                        .get(&who)
                        .is_some_and(|(held, _)| held.is_some());
                self.photos.held.insert(who, (photo, due));
                if changed {
                    for c in self.clients.values_mut() {
                        c.dirty = true;
                    }
                }
            }
            PhotosEvent::Failed(who) => {
                self.photos.loading.remove(&who);
                // One held (its cache emptied underneath) waits as one
                // never loaded does.
                if let Some((_, due)) = self.photos.held.get_mut(&who) {
                    *due = now + RETRY_MS;
                }
                self.photos.failed.insert(who, now);
            }
        }
    }
}
