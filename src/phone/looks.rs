//! The pictures a tab's Claude showed with `switchboard look`, for its
//! channel on the phone (issue 287): as `look` runs, off the server's
//! task, each picture the phone can draw (`images::kind`) is copied into
//! the phone's images folder under a name made from its contents
//! (`images::keep_copy`), so the phone reaches it by that name through
//! `/api/images/<name>` as it does the images you send, never by its path,
//! and the copies go with the folder's 30-day removal. The server keeps
//! each look on the tab's Claude pane with the transcript it was shown in
//! (`serve`'s `Phone::looks`), lists the current transcript's in the
//! stream `watch` sends, and `phone::web` merges them into the channel's
//! messages by time (`merge`). A video, a PDF or any other file adds
//! nothing. Settings' `phone_look_pictures` off, nothing is copied.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::images;
use super::turns::Message;

/// How many looks a pane keeps, the newest.
pub const MOST: usize = 80;

/// One `look`: when it ran (ms), the copies' names in the images folder,
/// in the order shown, and whether they are a before and after.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Look {
    pub at: i64,
    pub images: Vec<String>,
    pub before_after: bool,
}

/// A look kept on a tab's Claude pane, with the transcript it was shown
/// in: a new conversation in the pane has none of the last one's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kept {
    pub transcript: PathBuf,
    pub look: Look,
}

/// Copies the pictures among `files` the phone can draw into `dir`; `None`
/// when there are none. A before and after with one of its two left out
/// is a picture alone.
pub fn copy(dir: &Path, files: &[PathBuf], before_after: bool, at: i64) -> Option<Look> {
    let images: Vec<String> = files
        .iter()
        .filter_map(|f| images::keep_copy(dir, f))
        .collect();
    if images.is_empty() {
        return None;
    }
    Some(Look {
        at,
        before_after: before_after && images.len() == 2,
        images,
    })
}

/// A look as a message of Claude's in the channel, labeled as its showing,
/// a before and after's two titled as the side panel titles them.
pub fn message(look: &Look) -> Message {
    let text = match look.images.len() {
        _ if look.before_after => "Showed a before and after".to_string(),
        1 => "Showed a picture".to_string(),
        n => format!("Showed {n} pictures"),
    };
    let titles = if look.before_after {
        vec!["Before".to_string(), "After".to_string()]
    } else {
        Vec::new()
    };
    Message {
        who: "look",
        text,
        images: look.images.clone(),
        titles,
        at: look.at,
        from: String::new(),
        queued: false,
        offset: 0,
    }
}

/// A window's `messages` (`turns::Window`) with `looks` among them by
/// time, each after the messages written before it ran, with that
/// message's offset (the window's `from` before them all). A look from
/// before the first message of a window with turns before it (`earlier`)
/// belongs to those turns, and one from the first time the turns after it
/// show on (`until`, which the app sends) to those, so each is left out;
/// a window with turns before it and no message that says when takes
/// none, the next window back taking them.
pub fn merge(
    mut messages: Vec<Message>,
    looks: &[Look],
    from: u64,
    earlier: bool,
    until: Option<i64>,
) -> Vec<Message> {
    let first = earlier
        .then(|| messages.iter().map(|m| m.at).find(|at| *at != 0))
        .flatten();
    if earlier && first.is_none() {
        return messages;
    }
    for look in looks {
        if first.is_some_and(|first| look.at < first) || until.is_some_and(|u| look.at >= u) {
            continue;
        }
        let at = messages
            .iter()
            .position(|m| m.at != 0 && m.at > look.at)
            .unwrap_or(messages.len());
        let mut shown = message(look);
        shown.offset = at.checked_sub(1).map_or(from, |i| messages[i].offset);
        messages.insert(at, shown);
    }
    messages
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(who: &'static str, text: &str, at: i64) -> Message {
        Message {
            who,
            text: text.into(),
            images: Vec::new(),
            titles: Vec::new(),
            at,
            from: String::new(),
            queued: false,
            offset: at as u64,
        }
    }

    fn look(at: i64, n: usize, before_after: bool) -> Look {
        Look {
            at,
            images: (0..n).map(|i| format!("{i}.png")).collect(),
            before_after,
        }
    }

    #[test]
    fn a_look_is_labeled_as_claudes_showing() {
        assert_eq!(message(&look(1, 1, false)).text, "Showed a picture");
        assert_eq!(message(&look(1, 2, false)).text, "Showed 2 pictures");
        let both = message(&look(1, 2, true));
        assert_eq!(both.text, "Showed a before and after");
        assert_eq!(both.titles, ["Before", "After"]);
        assert_eq!(both.who, "look");
    }

    #[test]
    fn looks_go_among_the_messages_by_time() {
        let messages = vec![
            said("you", "show me", 10),
            said("tool", "Bash: switchboard look a.png", 20),
            said("claude", "Here they are", 40),
        ];
        let merged = merge(messages, &[look(30, 2, false)], 0, false, None);
        let order: Vec<&str> = merged.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(
            order,
            [
                "show me",
                "Bash: switchboard look a.png",
                "Showed 2 pictures",
                "Here they are"
            ]
        );
        assert_eq!(merged[2].images, ["0.png", "1.png"]);
    }

    #[test]
    fn a_look_outside_the_window_is_left_to_the_turns_it_came_in() {
        let messages = vec![said("you", "a", 10), said("claude", "b", 20)];
        // Turns before the window: one from before its first message is theirs.
        let merged = merge(messages.clone(), &[look(5, 1, false)], 10, true, None);
        assert_eq!(merged, messages);
        let merged = merge(messages.clone(), &[look(5, 1, false)], 10, false, None);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].offset, 10);
        // Turns after it: one from when they started on is theirs.
        let merged = merge(messages.clone(), &[look(30, 1, false)], 10, false, Some(30));
        assert_eq!(merged, messages);
        let merged = merge(messages, &[look(25, 1, false)], 10, false, Some(30));
        assert_eq!(merged.len(), 3);
        assert_eq!((merged[2].who, merged[2].offset), ("look", 20));
        // Turns before it and no time to tell them by: the next window back takes it.
        let untimed = vec![said("tool", "Bash: ls", 0)];
        assert_eq!(
            merge(untimed.clone(), &[look(5, 1, false)], 10, true, Some(30)),
            untimed
        );
    }

    #[test]
    fn only_the_pictures_the_phone_draws_are_copied() {
        let tmp = std::env::temp_dir().join(format!(
            "sbphone-looks-{}-{}",
            std::process::id(),
            crate::util::now_ms()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let dir = tmp.join("images");
        let png = tmp.join("a.png");
        let jpg = tmp.join("b.jpg");
        let video = tmp.join("demo.mp4");
        std::fs::write(&png, b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR").unwrap();
        std::fs::write(&jpg, b"\xff\xd8\xff\xe0rest").unwrap();
        std::fs::write(&video, b"\0\0\0\x18ftypmp42").unwrap();
        let both = copy(&dir, &[png.clone(), jpg], true, 7).unwrap();
        assert_eq!(both.images.len(), 2);
        assert!(both.before_after);
        for name in &both.images {
            assert!(images::path_of(&dir, name).is_some(), "{name}");
        }
        assert_eq!(copy(&dir, std::slice::from_ref(&video), false, 7), None);
        let one = copy(&dir, &[png, video], true, 7).unwrap();
        assert_eq!((one.images.len(), one.before_after), (1, false));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
