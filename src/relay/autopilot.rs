//! Autopilot's settings as a Slack form that also starts it (UX 2.0 board
//! 52), opened from the Home tab's Autopilot button while autopilot is off.
//! No file of the Node relay is behind it.
//!
//! The form holds what the settings box does that matters from a phone:
//! the note it works, the branch it lands on, the tabs at once, the spend
//! limit a day, and its switches (Planned only, Hands-free, Land without
//! asking), each starting at autopilot's settings as they are. Its Start
//! sends them to the server's `autopilot-start`, which checks and saves
//! them as the settings box's Start does, and the relay says in the DM
//! whether it started. The repository, the most tabs open and the skipped
//! items are kept as they were, so the form needs autopilot set up once on
//! the computer, which picks the repository; until then it says so.
//!
//! A note's name is something of the person's own, so the form names notes
//! only with Settings' "Notes in Slack" (`notes_in_slack`) on, and then
//! only their names: their items stay on the computer. Off, it names none,
//! starts on the note autopilot last worked, and never says its name.
//!
//! The form, like every form, is taken only from the owner
//! (`slack::submission_from`).

use anyhow::Result;
use serde_json::{Value, json};

use super::Cx;
use super::post::{PostOpts, post};
use super::slack::Form;
use crate::autopilot::Mode;
use crate::autopilot::mode::{self, NoteChoices, note_title};
use crate::autopilot::settings_box::Fields;
use crate::util;

/// The setting that lets the form name notes.
pub const NOTES_IN_SLACK: &str = "notes_in_slack";

/// The value of the default branch among "Lands on"'s options, which no
/// branch can be named (git refuses a `:` in one), since Slack takes no
/// empty value.
const DEFAULT_BRANCH: &str = ":default";

/// The most options a Slack menu takes.
const MOST_OPTIONS: usize = 100;

/// The longest value Slack takes for an option.
const LONGEST_VALUE: usize = 150;

/// What the form is drawn from, read off the relay's thread.
#[derive(Clone, Debug, Default)]
pub struct Shown {
    pub mode: Mode,
    /// The notes to pick from, with Notes in Slack on.
    pub notes: Option<NoteChoices>,
    /// The repository's default branch.
    pub default: Option<String>,
    /// The other branches to land on: the ones picked before, then the
    /// repository's local ones by name.
    pub branches: Vec<String>,
    /// Whether Settings allows Hands-free (`autopilot_handsfree`), which
    /// the form offers only then, or while it is on already, as the
    /// settings box does.
    pub offer_hands_free: bool,
}

/// Whether autopilot is allowed at all (Settings' Autopilot, on at first).
pub fn allowed(cx: &Cx) -> bool {
    (cx.settings)("autopilot") != Some(Value::Bool(false))
}

/// Whether Settings allows Hands-free (on at first).
pub fn hands_free_offered(cx: &Cx) -> bool {
    (cx.settings)("autopilot_handsfree") != Some(Value::Bool(false))
}

/// Whether the form may name notes.
pub fn names_notes(cx: &Cx) -> bool {
    (cx.settings)(NOTES_IN_SLACK) == Some(Value::Bool(true))
}

/// Whether the Home tab offers the form: autopilot allowed and off.
pub fn offered(cx: &Cx) -> bool {
    allowed(cx) && !Mode::read(&cx.paths.switchboard_state).on
}

/// The mode, the notes (only when they may be named) and the branches,
/// read from the files.
pub fn read(state: &std::path::Path, notes: &std::path::Path, named: bool) -> Shown {
    let mode = Mode::read(state);
    let notes = named.then(|| mode::note_choices(notes, &mode.note));
    let common = mode
        .set_up()
        .then(|| crate::tabs::git::locate(&mode.repo).map(|p| p.common_dir))
        .flatten();
    let (default, branches) = match common.as_deref() {
        Some(c) => {
            let default = crate::tabs::git::default_branch(c);
            let mut local = crate::tabs::git::local_branches(c);
            let recent = crate::cmd::work_recent::branches(c);
            // A branch only origin has, picked before, stays listed while
            // origin has it, as the settings box lists it.
            let remote_only: Vec<String> = recent
                .iter()
                .filter(|b| !b.is_empty() && !local.contains(b))
                .filter(|b| crate::tabs::git::on_origin(c, b))
                .cloned()
                .collect();
            local.extend(remote_only);
            let picks = mode::target_choices(default.as_deref(), &local, &recent);
            let branches = picks
                .into_iter()
                .map(|(_, value)| value)
                .filter(|v| !v.is_empty() && v != mode::ANOTHER_BRANCH)
                .collect();
            (default, branches)
        }
        None => (None, Vec::new()),
    };
    Shown {
        mode,
        notes,
        default,
        branches,
        offer_hands_free: false,
    }
}

fn plain(text: &str) -> Value {
    json!({ "type": "plain_text", "text": text })
}

fn option(text: &str, value: &str) -> Value {
    json!({ "text": plain(&super::screens::clip(text, 75)), "value": value })
}

fn context(text: &str) -> Value {
    json!({ "type": "context", "elements": [{ "type": "mrkdwn", "text": text }] })
}

/// An optional text field for what a menu has no room for.
fn typed(id: &str, label: &str, hint: &str) -> Value {
    json!({
        "type": "input", "block_id": id, "optional": true,
        "label": plain(label), "hint": plain(hint),
        "element": { "type": "plain_text_input", "action_id": id, "max_length": 200 },
    })
}

/// A menu field, its options' first chosen when `chosen` isn't among them.
fn menu(id: &str, label: &str, options: Vec<Value>, chosen: &str) -> Value {
    let initial = options
        .iter()
        .find(|o| o["value"] == chosen)
        .or(options.first())
        .cloned();
    let mut element = json!({ "type": "static_select", "action_id": id, "options": options });
    if let Some(initial) = initial {
        element["initial_option"] = initial;
    }
    json!({ "type": "input", "block_id": id, "label": plain(label), "element": element })
}

/// The form, or, while autopilot isn't set up (or has no note to work and
/// may name none), a screen saying what to do on the computer first.
pub fn form(shown: &Shown) -> Value {
    let mode = &shown.mode;
    let mut blocks = Vec::new();
    let unset = if !mode.set_up() {
        Some(
            "Set autopilot up once on the computer (M-p's \"Autopilot: work a note\"), which picks the repository it works in; then start it from here.",
        )
    } else if shown.notes.is_none() && mode.note.is_empty() {
        Some(
            "Autopilot has no note to work yet. Start it once on the computer, or turn on Settings' \"Notes in Slack\" to pick a note here.",
        )
    } else {
        None
    };
    if let Some(unset) = unset {
        blocks.push(json!({ "type": "section", "text": { "type": "mrkdwn", "text": unset } }));
        return json!({
            "type": "modal",
            "callback_id": "autopilot-unset",
            "title": plain("Autopilot"),
            "close": plain("Done"),
            "blocks": blocks,
        });
    }
    match &shown.notes {
        Some(notes) => {
            // The note it last worked leads, so Start with nothing changed
            // keeps it however many notes there are.
            let current = (!mode.note.is_empty()).then_some(&mode.note);
            let all: Vec<&String> = current
                .into_iter()
                .chain(notes.open.iter().chain(&notes.rest).filter(|n| **n != mode.note))
                .collect();
            let fits = |n: &&String| n.len() <= LONGEST_VALUE;
            // Any note left out, past the menu's room or too long a path
            // for a Slack value, can be typed instead.
            let cut = all.len() > MOST_OPTIONS || !all.iter().all(fits);
            let all: Vec<&String> = all.into_iter().filter(fits).collect();
            let options: Vec<Value> = all
                .into_iter()
                .take(MOST_OPTIONS)
                // By its path within the notes, as the settings box names
                // it, so notes of one name in two folders stand apart.
                .map(|n| option(n.strip_suffix(".md").unwrap_or(n), n))
                .collect();
            blocks.push(menu("note", "Works the note", options, &mode.note));
            if cut {
                blocks.push(typed(
                    "note_typed",
                    "Or another note",
                    "Its path within your notes, for one the menu has no room for. Used instead of the menu.",
                ));
            }
            blocks.push(context(
                "Only the note's name reaches Slack, and only with Settings' \"Notes in Slack\" on; its items stay on the computer.",
            ));
        }
        None => blocks.push(context(
            "Works the note it last worked. Turn on Settings' \"Notes in Slack\" to pick another here; only notes' names would reach Slack.",
        )),
    }
    let lands = |b: &str| format!("{b}, through queue-and-clean");
    let mut targets = vec![option(
        &lands(shown.default.as_deref().unwrap_or("The default branch")),
        DEFAULT_BRANCH,
    )];
    // The branch it lands on now leads the rest even when only origin has
    // it, so Start with nothing changed keeps it.
    let current = (!mode.target.is_empty()).then_some(&mode.target);
    let branches: Vec<&String> = current
        .into_iter()
        .chain(shown.branches.iter().filter(|b| **b != mode.target))
        .filter(|b| b.len() <= LONGEST_VALUE)
        .collect();
    targets.extend(
        branches
            .into_iter()
            .take(MOST_OPTIONS - 1)
            .map(|b| option(&lands(b), b)),
    );
    let target = if mode.target.is_empty() {
        DEFAULT_BRANCH
    } else {
        &mode.target
    };
    blocks.push(menu("target", "Lands on", targets, target));
    // Any other branch, origin's included, as the settings box's
    // "Another branch…" takes one.
    blocks.push(typed(
        "target_typed",
        "Or another branch",
        "Any other branch, one only origin has included. Used instead of the menu.",
    ));
    // Up to the most tabs open, which the form keeps as it is: more at once
    // would raise it.
    let counts: Vec<u32> = (1..=mode.cap.max(mode.slots).max(1)).collect();
    let counts = counts
        .iter()
        .map(|n| option(&n.to_string(), &n.to_string()))
        .collect();
    blocks.push(menu(
        "slots",
        "Tabs at once",
        counts,
        &mode.slots.to_string(),
    ));
    let mut limit = json!({ "type": "plain_text_input", "action_id": "limit", "max_length": 40, "placeholder": plain("No limit") });
    if mode.spend_limit > 0.0 {
        // In cents, as the board shows it, when that is exactly the limit;
        // else as stored, so Start never rounds it.
        let cents = format!("{:.2}", mode.spend_limit);
        let shown = if cents.parse::<f64>().ok() == Some(mode.spend_limit) {
            cents
        } else {
            crate::autopilot::settings_box::limit_text(mode.spend_limit)
        };
        limit["initial_value"] = json!(format!("${shown}"));
    }
    blocks.push(json!({
        "type": "input", "block_id": "limit", "optional": true,
        "label": plain("Spend limit a day"),
        "hint": plain("It pauses once today's spend reaches it."),
        "element": limit,
    }));
    let switch = |value: &str, label: &str, about: &str| json!({ "text": plain(label), "description": plain(about), "value": value });
    let switches = [
        (
            mode.ready_only,
            switch(
                "planned",
                "Planned only",
                "Takes only items with a settled plan.",
            ),
        ),
        (
            mode.hands_free,
            switch(
                "hands-free",
                "Hands-free",
                "Answers its tabs' questions itself.",
            ),
        ),
        (
            mode.land_without_asking || mode.hands_free,
            switch(
                "land",
                "Land without asking",
                "Pushes once tests, two clean reviews and leak-check pass.",
            ),
        ),
    ];
    let offered =
        |s: &Value| s["value"] != "hands-free" || shown.offer_hands_free || mode.hands_free;
    let switches: Vec<(bool, Value)> = switches.into_iter().filter(|(_, s)| offered(s)).collect();
    let mut element = json!({
        "type": "checkboxes", "action_id": "switches",
        "options": switches.iter().map(|(_, s)| s.clone()).collect::<Vec<_>>(),
    });
    let on: Vec<Value> = switches
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, s)| s.clone())
        .collect();
    if !on.is_empty() {
        element["initial_options"] = json!(on);
    }
    blocks.push(json!({
        "type": "input", "block_id": "switches", "optional": true,
        "label": plain("Switches"),
        "element": element,
    }));
    json!({
        "type": "modal",
        "callback_id": "autopilot",
        // Whether it started landing without asking, for reading an
        // unticked Land without asking as the form showed it, and whether
        // it changes autopilot at work rather than starting it.
        "private_metadata": util::json_compact(&json!({ "landing": mode.land_without_asking || mode.hands_free, "change": mode.on })),
        "title": plain("Autopilot"),
        "submit": plain(if mode.on { "Save" } else { "Start" }),
        "close": plain("Cancel"),
        "blocks": blocks,
    })
}

/// The fields the form's Start sends, from autopilot's settings as they
/// are and what was chosen in the form; the note chosen only when the form
/// may name notes, else the one it last worked.
pub fn fields_of(mode: &Mode, form: &Form, named: bool, hands_free: bool) -> Fields {
    let mut fields = Fields::from_mode(mode);
    let typed_note = form.value("note_typed").trim();
    if named && !typed_note.is_empty() {
        fields.note = if typed_note.ends_with(".md") {
            typed_note.to_string()
        } else {
            format!("{typed_note}.md")
        };
    } else if named && !form.value("note").is_empty() {
        fields.note = form.value("note").to_string();
    }
    let typed_target = form.value("target_typed").trim();
    fields.target = match form.value("target") {
        _ if !typed_target.is_empty() => typed_target.to_string(),
        DEFAULT_BRANCH | "" => String::new(),
        branch => branch.to_string(),
    };
    if let Ok(n) = form.value("slots").parse::<u32>() {
        fields.slots = n.clamp(1, fields.cap.max(fields.slots).max(1));
    }
    fields.limit = form.value("limit").trim().to_string();
    let switches: Vec<&str> = form.value("switches").split(',').collect();
    fields.ready_only = switches.contains(&"planned");
    // Hands-free only while Settings allows it, or it is on already.
    fields.hands_free = switches.contains(&"hands-free") && (hands_free || mode.hands_free);
    // Land without asking unticked where it started ticked turns Hands-free
    // off with it, as turning it off in the settings box does; Hands-free
    // ticked otherwise lands without asking.
    let land = switches.contains(&"land");
    let was_landing = form
        .meta
        .get("landing")
        .and_then(Value::as_bool)
        .unwrap_or(mode.land_without_asking || mode.hands_free);
    if !land && was_landing {
        fields.hands_free = false;
    }
    fields.land_without_asking = land || fields.hands_free;
    fields
}

/// The Home tab's Autopilot: the form, opened on Slack's trigger.
pub async fn open(cx: &Cx, trigger: &str) -> Result<()> {
    let state = cx.paths.switchboard_state.clone();
    let notes = cx.paths.notes.clone();
    let named = names_notes(cx);
    let mut shown = tokio::task::spawn_blocking(move || read(&state, &notes, named))
        .await
        .unwrap_or_default();
    shown.offer_hands_free = hands_free_offered(cx);
    cx.slack
        .api(
            "views.open",
            json!({ "trigger_id": trigger, "view": form(&shown) }),
        )
        .await?;
    Ok(())
}

/// The form's Start: autopilot started through the server, and the DM told
/// whether it did, naming the note only when the form may; or, opened
/// while it worked (the Home tab's Settings, board 81), its Save: its
/// settings changed, and the DM told what changed.
pub async fn submitted(cx: &Cx, form: &Form) -> Result<()> {
    let named = names_notes(cx);
    let mode = Mode::read(&cx.paths.switchboard_state);
    let change = form.meta["change"] == json!(true);
    let said = if !allowed(cx) {
        "🤖 Autopilot is off in Settings (Autopilot), so it didn't start.".to_string()
    } else if change && !mode.on {
        "🤖 Autopilot has stopped, so nothing changed.".to_string()
    } else if change {
        let fields = fields_of(&mode, form, named, hands_free_offered(cx));
        let hidden = (!named).then(|| note_title(&fields.note));
        let words = |text: &str| match &hidden {
            Some(title) if !title.is_empty() => unnamed(text, title),
            _ => text.to_string(),
        };
        match cx
            .mux
            .start_autopilot(util::json_compact(&json!(fields)), true)
            .await
        {
            Ok(changed) => {
                let changed = changed.trim();
                format!(
                    "🤖 Autopilot: {}",
                    words(changed.strip_prefix("Autopilot: ").unwrap_or(changed))
                )
            }
            Err(e) => format!("🤖 Autopilot didn't change: {}", words(&e.to_string())),
        }
    } else if mode.on {
        "🤖 Autopilot is already on.".to_string()
    } else {
        let fields = fields_of(&mode, form, named, hands_free_offered(cx));
        let hidden = (!named).then(|| note_title(&fields.note));
        let words = |text: &str| match &hidden {
            Some(title) if !title.is_empty() => unnamed(text, title),
            _ => text.to_string(),
        };
        match cx
            .mux
            .start_autopilot(util::json_compact(&json!(fields)), false)
            .await
        {
            Ok(started) => format!("🤖 Autopilot: {}", words(started.trim())),
            Err(e) => format!("🤖 Autopilot didn't start: {}", words(&e.to_string())),
        }
    };
    post(cx, &said, None, PostOpts::default()).await?;
    super::dashboard::refresh_dashboard(cx, super::dashboard::Refresh::opened_quick()).await
}

/// The server's words with the note's name taken out where they name it
/// ("Working <note>:", "the note <note>"), and nothing else touched, so a
/// repository or branch whose name holds the same word reads as it is.
fn unnamed(text: &str, title: &str) -> String {
    let text = match text.strip_prefix(&format!("Working {title}:")) {
        Some(rest) => format!("Working its note:{rest}"),
        None => text.to_string(),
    };
    text.replace(&format!("the note {title}"), "its note")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn set_up() -> Mode {
        Mode {
            note: "Work/Plans.md".into(),
            repo: "/r".into(),
            slots: 2,
            cap: 4,
            spend_limit: 20.0,
            ready_only: true,
            target: "release".into(),
            ..Mode::default()
        }
    }

    fn block<'a>(view: &'a Value, id: &str) -> &'a Value {
        view["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["block_id"] == id)
            .unwrap_or(&Value::Null)
    }

    #[test]
    fn the_form_starts_at_autopilots_settings() {
        let shown = Shown {
            mode: set_up(),
            notes: Some(NoteChoices {
                open: vec!["Work/Plans.md".into(), "Ideas.md".into()],
                rest: vec![],
            }),
            default: Some("main".into()),
            branches: vec!["release".into()],
            offer_hands_free: true,
        };
        let view = form(&shown);
        assert_eq!(view["callback_id"], "autopilot");
        let note = &block(&view, "note")["element"];
        assert_eq!(note["initial_option"]["text"]["text"], "Work/Plans");
        assert_eq!(note["options"][1]["value"], "Ideas.md");
        // The note it last worked leads past the most options Slack takes,
        // ticked through and behind a hundred notes with items left.
        let many = form(&Shown {
            notes: Some(NoteChoices {
                open: (0..150).map(|n| format!("n{n}.md")).collect(),
                rest: vec!["Work/Plans.md".into()],
            }),
            ..shown.clone()
        });
        let many = &block(&many, "note")["element"];
        assert_eq!(many["initial_option"]["value"], "Work/Plans.md");
        assert_eq!(many["options"].as_array().unwrap().len(), MOST_OPTIONS);
        assert!(block(&view, "note_typed").is_null());
        assert!(!block(&view, "target_typed").is_null());
        // Past them, a field takes one the menu has no room for.
        let cut = form(&Shown {
            notes: Some(NoteChoices {
                open: (0..150).map(|n| format!("n{n}.md")).collect(),
                rest: vec![],
            }),
            branches: (0..150).map(|n| format!("b{n}")).collect(),
            ..shown.clone()
        });
        assert!(!block(&cut, "note_typed").is_null());
        let long = form(&Shown {
            notes: Some(NoteChoices {
                open: vec!["Work/Plans.md".into(), format!("{}.md", "x".repeat(160))],
                rest: vec![],
            }),
            ..shown.clone()
        });
        assert!(!block(&long, "note_typed").is_null());
        assert!(!block(&cut, "target_typed").is_null());
        // Tabs at once goes up to the most tabs open, which it keeps.
        let counts = block(&view, "slots")["element"]["options"]
            .as_array()
            .unwrap()
            .len();
        assert_eq!(counts, 4);
        let target = &block(&view, "target")["element"];
        assert_eq!(
            target["options"][0]["text"]["text"],
            "main, through queue-and-clean"
        );
        assert_eq!(target["initial_option"]["value"], "release");
        // Past the most options Slack takes, it still leads them.
        let many = form(&Shown {
            branches: (0..150)
                .map(|n| format!("b{n}"))
                .chain(["release".to_string()])
                .collect(),
            ..shown.clone()
        });
        let many = &block(&many, "target")["element"];
        assert_eq!(many["initial_option"]["value"], "release");
        assert_eq!(many["options"].as_array().unwrap().len(), MOST_OPTIONS);
        // A branch only origin has, which isn't among the local ones, is kept.
        let only_origin = form(&Shown {
            branches: vec![],
            ..shown.clone()
        });
        assert_eq!(
            block(&only_origin, "target")["element"]["initial_option"]["value"],
            "release"
        );
        assert_eq!(
            block(&view, "slots")["element"]["initial_option"]["value"],
            "2"
        );
        assert_eq!(block(&view, "limit")["element"]["initial_value"], "$20.00");
        // A limit finer than a cent is shown as stored, never rounded.
        let mut fine = shown.clone();
        fine.mode.spend_limit = 0.004;
        assert_eq!(
            block(&form(&fine), "limit")["element"]["initial_value"],
            "$0.004"
        );
        let on = block(&view, "switches")["element"]["initial_options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["value"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(on, ["planned"]);
    }

    #[test]
    fn without_notes_in_slack_the_form_names_no_note() {
        let shown = Shown {
            mode: set_up(),
            default: Some("main".into()),
            ..Shown::default()
        };
        let view = form(&shown);
        assert!(block(&view, "note").is_null());
        assert!(!view.to_string().contains("Plans"));
        // Hands-free off in Settings: the switch isn't offered.
        assert!(!view.to_string().contains("hands-free"));
    }

    #[test]
    fn the_notes_name_is_taken_out_of_what_the_server_says_and_nothing_else() {
        assert_eq!(
            unnamed(
                "Working Work: 2 at a time, in ~/Work, landing on team/Work",
                "Work"
            ),
            "Working its note: 2 at a time, in ~/Work, landing on team/Work"
        );
        assert_eq!(
            unnamed("Couldn't read the note Work, pick another: gone", "Work"),
            "Couldn't read its note, pick another: gone"
        );
    }

    #[test]
    fn autopilot_not_set_up_is_said_rather_than_asked() {
        let view = form(&Shown::default());
        assert_eq!(view["callback_id"], "autopilot-unset");
        assert!(view.get("submit").is_none());
    }

    #[test]
    fn the_fields_are_what_was_chosen_over_the_settings() {
        let values = HashMap::from([
            ("note".to_string(), "Ideas.md".to_string()),
            ("target".to_string(), DEFAULT_BRANCH.to_string()),
            ("slots".to_string(), "3".to_string()),
            ("limit".to_string(), " $5 ".to_string()),
            ("switches".to_string(), "hands-free".to_string()),
        ]);
        let submitted = Form {
            kind: "autopilot".into(),
            values,
            ..Form::default()
        };
        let fields = fields_of(&set_up(), &submitted, true, true);
        assert_eq!(fields.note, "Ideas.md");
        assert_eq!(fields.target, "");
        assert_eq!((fields.slots, fields.cap), (3, 4));
        // More at once than the most open is held to it.
        let mut more = submitted.clone();
        more.values.insert("slots".into(), "9".into());
        assert_eq!(fields_of(&set_up(), &more, true, true).slots, 4);
        assert_eq!(fields.limit, "$5");
        assert!(!fields.ready_only && fields.hands_free && fields.land_without_asking);
        assert_eq!(fields.repo, crate::paths::tilde(std::path::Path::new("/r")));
        // Without Notes in Slack, the note it last worked, whatever is sent.
        assert_eq!(
            fields_of(&set_up(), &submitted, false, true).note,
            "Work/Plans.md"
        );
        // What is typed for a note or branch the menus had no room for wins.
        let mut typed = submitted.clone();
        typed
            .values
            .insert("note_typed".into(), " Later/Far ".into());
        typed
            .values
            .insert("target_typed".into(), "team/far".into());
        let far = fields_of(&set_up(), &typed, true, true);
        assert_eq!(
            (far.note.as_str(), far.target.as_str()),
            ("Later/Far.md", "team/far")
        );
        assert_eq!(
            fields_of(&set_up(), &typed, false, true).note,
            "Work/Plans.md"
        );
        // Land without asking unticked from on turns Hands-free off too.
        let mut freed = set_up();
        freed.hands_free = true;
        freed.land_without_asking = true;
        let only_hf = fields_of(&freed, &submitted, true, true);
        assert!(!only_hf.hands_free && !only_hf.land_without_asking);
        // As the form showed it, though the settings changed since it opened.
        let mut stale = submitted.clone();
        stale.meta = json!({ "landing": true });
        let as_shown = fields_of(&set_up(), &stale, true, true);
        assert!(!as_shown.hands_free && !as_shown.land_without_asking);
        // Hands-free off in Settings: a submitted Hands-free is dropped.
        let off = fields_of(&set_up(), &submitted, true, false);
        assert!(!off.hands_free && !off.land_without_asking);
    }
}
