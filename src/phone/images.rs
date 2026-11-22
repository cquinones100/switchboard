//! Images sent from the phone's photo library to a tab's Claude (board
//! 174): each kept in `~/.local/state/switchboard/phone/images/`, a folder
//! only you can read, under a name made here (a random ID and the
//! extension of the type its first bytes show), never one the phone sends;
//! only a PNG, JPEG, GIF or WebP up to `MAX_BYTES`, whatever the request
//! says its type is. A message names its images by those names, and each
//! one's full path is typed on a line of its own after the text, so Claude
//! reads it; the channel finds those lines in the transcript again and
//! draws them as the images. Images older than `KEEP` are removed as the
//! phone server starts.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Result, anyhow};

/// The largest image kept.
pub const MAX_BYTES: usize = 20 * 1024 * 1024;
/// How long an image is kept.
pub const KEEP: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// How long a copy cut off as it was written is kept.
const PART_KEEP: Duration = Duration::from_secs(60 * 60);
/// The hex digits of a name's random ID.
const ID_LEN: usize = 32;

/// The folder the images are kept in.
pub fn dir() -> PathBuf {
    crate::paths::state_dir().join("phone/images")
}

/// The extension of the image type `bytes` start as, if they are one taken.
pub fn kind(bytes: &[u8]) -> Option<&'static str> {
    let at = |from: usize, magic: &[u8]| bytes.get(from..from + magic.len()) == Some(magic);
    if at(0, b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if at(0, b"\xff\xd8\xff") {
        Some("jpg")
    } else if at(0, b"GIF87a") || at(0, b"GIF89a") {
        Some("gif")
    } else if at(0, b"RIFF") && at(8, b"WEBP") {
        Some("webp")
    } else {
        None
    }
}

/// The content type an image's name is served as.
pub fn content_type(name: &str) -> &'static str {
    match name.rsplit('.').next() {
        Some("png") => "image/png",
        Some("jpg") => "image/jpeg",
        Some("gif") => "image/gif",
        _ => "image/webp",
    }
}

/// Whether `name` has the shape of a name `save` makes: 32 lowercase hex
/// digits and a taken extension, so no other file, and nothing outside the
/// folder, is ever reached by one.
pub fn is_name(name: &str) -> bool {
    let Some((id, ext)) = name.split_once('.') else {
        return false;
    };
    id.len() == ID_LEN
        && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        && matches!(ext, "png" | "jpg" | "gif" | "webp")
}

/// The path of the image named `name` in `dir`, if it is one kept there.
pub fn path_of(dir: &Path, name: &str) -> Option<PathBuf> {
    let path = dir.join(name);
    (is_name(name) && path.is_file()).then_some(path)
}

/// Keeps `bytes` in `dir` under a name made from `random` (16 bytes), and
/// gives the name; refuses what isn't a taken image or is too large,
/// writing nothing.
pub fn save(dir: &Path, bytes: &[u8], random: &[u8]) -> Result<String> {
    if bytes.len() > MAX_BYTES {
        return Err(anyhow!("an image can't be over 20 MB"));
    }
    let ext = kind(bytes).ok_or_else(|| anyhow!("only a PNG, JPEG, GIF or WebP image is taken"))?;
    let id: String = random.iter().map(|b| format!("{b:02x}")).collect();
    let name = format!("{id}.{ext}");
    if !is_name(&name) {
        return Err(anyhow!("no name could be made for the image"));
    }
    make_dir(dir)?;
    let mut file = owner_only().create_new(true).open(dir.join(&name))?;
    file.write_all(bytes)?;
    Ok(name)
}

/// Keeps a copy of the picture at `file` in `dir`, named after its
/// contents (so a picture shown twice is kept once, its time renewed for
/// `prune`), and gives the name; `None` for what isn't a taken image, is
/// too large or can't be read.
pub fn keep_copy(dir: &Path, file: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    if fs::metadata(file).ok()?.len() > MAX_BYTES as u64 {
        return None;
    }
    let bytes = fs::read(file).ok()?;
    let ext = kind(&bytes)?;
    let digest = Sha256::digest(&bytes);
    let id = &digest[..ID_LEN / 2];
    let name = format!(
        "{}.{ext}",
        id.iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
    let kept = dir.join(&name);
    // One of another length is a copy cut short, written again.
    let whole = fs::metadata(&kept).is_ok_and(|m| m.is_file() && m.len() == bytes.len() as u64);
    if whole {
        let renewed = fs::File::options()
            .write(true)
            .open(&kept)
            .and_then(|f| f.set_modified(SystemTime::now()));
        return renewed.is_ok().then_some(name);
    }
    // Written beside it under a name no request reaches, then renamed into
    // place, so a write cut short never leaves a copy under the name.
    make_dir(dir).ok()?;
    let part = dir.join(format!(".{name}.part"));
    let written = owner_only()
        .create(true)
        .truncate(true)
        .open(&part)
        .and_then(|mut f| f.write_all(&bytes))
        .and_then(|()| fs::rename(&part, &kept));
    if written.is_err() {
        let _ = fs::remove_file(&part);
        return None;
    }
    Some(name)
}

#[cfg(unix)]
fn make_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    // A folder made before, by hand or by an older build, is closed too.
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn make_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)?;
    Ok(())
}

fn owner_only() -> fs::OpenOptions {
    let mut options = fs::OpenOptions::new();
    options.write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options
}

/// Removes the images in `dir` last changed more than `KEEP` before `now`,
/// and a copy `keep_copy` was cut off writing more than `PART_KEEP` before.
pub fn prune(dir: &Path, now: SystemTime) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let part = name
            .strip_prefix('.')
            .and_then(|n| n.strip_suffix(".part"))
            .is_some_and(is_name);
        let keep = if part {
            PART_KEEP
        } else if is_name(&name) {
            KEEP
        } else {
            continue;
        };
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > keep);
        if old {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// What is typed into Claude for a message: its text, then each image's
/// full path on a line of its own.
pub fn typed(text: &str, paths: &[PathBuf]) -> String {
    let text = text.trim();
    std::iter::once(text.to_string())
        .filter(|t| !t.is_empty())
        .chain(paths.iter().map(|p| p.display().to_string()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A message you sent read back: its text without the lines that are the
/// path of an image in `dir`, and those images' names, in order.
pub fn split(text: &str, dir: &Path) -> (String, Vec<String>) {
    let mut words = Vec::new();
    let mut images = Vec::new();
    for line in text.lines() {
        let name = Path::new(line.trim())
            .strip_prefix(dir)
            .ok()
            .and_then(|rest| rest.to_str())
            .filter(|rest| is_name(rest));
        match name {
            Some(name) => images.push(name.to_string()),
            None => words.push(line),
        }
    }
    (words.join("\n").trim().to_string(), images)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
    const RANDOM: [u8; 16] = [0xab; 16];

    /// A folder of the test's own, removed when it ends.
    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Temp {
            let dir = std::env::temp_dir().join(format!(
                "sbphone-{name}-{}-{}",
                std::process::id(),
                crate::util::now_ms()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Temp(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn an_image_is_known_by_its_first_bytes() {
        assert_eq!(kind(PNG), Some("png"));
        assert_eq!(kind(b"\xff\xd8\xff\xe0rest"), Some("jpg"));
        assert_eq!(kind(b"GIF89a..."), Some("gif"));
        assert_eq!(kind(b"RIFF\0\0\0\0WEBPVP8 "), Some("webp"));
        assert_eq!(kind(b"<svg xmlns='http://www.w3.org/2000/svg'/>"), None);
        assert_eq!(kind(b"%PDF-1.7"), None);
        assert_eq!(kind(b""), None);
    }

    #[test]
    fn an_image_is_kept_under_a_name_made_here() {
        let tmp = Temp::new("kept");
        let dir = tmp.path().join("phone/images");
        let name = save(&dir, PNG, &RANDOM).unwrap();
        assert_eq!(name, format!("{}.png", "ab".repeat(16)));
        assert_eq!(fs::read(dir.join(&name)).unwrap(), PNG);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&dir), 0o700);
            assert_eq!(mode(&dir.join(&name)), 0o600);
        }
        assert_eq!(path_of(&dir, &name), Some(dir.join(&name)));
    }

    #[test]
    fn what_isnt_an_image_or_is_too_large_is_refused_and_nothing_written() {
        let tmp = Temp::new("refused");
        let dir = tmp.path().join("images");
        assert!(save(&dir, b"<html><script>alert(1)</script>", &RANDOM).is_err());
        let mut large = PNG.to_vec();
        large.resize(MAX_BYTES + 1, 0);
        assert!(save(&dir, &large, &RANDOM).is_err());
        assert!(!dir.exists());
    }

    #[test]
    fn only_a_name_of_the_folders_own_shape_is_reached() {
        let tmp = Temp::new("shape");
        let dir = tmp.path();
        let id = "0123456789abcdef0123456789abcdef";
        fs::write(dir.join(format!("{id}.png")), PNG).unwrap();
        fs::write(dir.join("notes.txt"), "x").unwrap();
        assert!(path_of(dir, &format!("{id}.png")).is_some());
        assert!(
            path_of(dir, &format!("{id}.jpg")).is_none(),
            "not a file there"
        );
        assert!(path_of(dir, "notes.txt").is_none());
        assert!(path_of(dir, &format!("../{id}.png")).is_none());
        assert!(path_of(dir, &format!("{}.png", id.to_uppercase())).is_none());
        assert!(path_of(dir, &format!("{id}.svg")).is_none());
        assert!(path_of(dir, &format!("{id}.png.png")).is_none());
    }

    #[test]
    fn the_paths_are_typed_after_the_text_a_line_each() {
        let paths = [PathBuf::from("/s/a.png"), PathBuf::from("/s/b.jpg")];
        assert_eq!(
            typed(" Wraps on my phone too \n", &paths),
            "Wraps on my phone too\n/s/a.png\n/s/b.jpg"
        );
        assert_eq!(typed("", &paths[..1]), "/s/a.png");
        assert_eq!(typed("just words", &[]), "just words");
    }

    #[test]
    fn a_sent_messages_image_lines_are_read_back_as_images() {
        let dir = Path::new("/home/me/.local/state/switchboard/phone/images");
        let a = "0123456789abcdef0123456789abcdef.png";
        let text = format!(
            "Wraps on my phone too\n{}\n/elsewhere/{a}\n{}/x.png",
            dir.join(a).display(),
            dir.display()
        );
        let (words, images) = split(&text, dir);
        assert_eq!(
            words,
            format!(
                "Wraps on my phone too\n/elsewhere/{a}\n{}/x.png",
                dir.display()
            )
        );
        assert_eq!(images, [a]);
        let (words, images) = split(&dir.join(a).display().to_string(), dir);
        assert_eq!((words.as_str(), images.len()), ("", 1));
    }

    #[test]
    fn a_picture_is_copied_once_under_its_contents_name() {
        let tmp = Temp::new("copy");
        let dir = tmp.path().join("images");
        let shot = tmp.path().join("shot.png");
        fs::write(&shot, PNG).unwrap();
        let name = keep_copy(&dir, &shot).unwrap();
        assert!(is_name(&name) && name.ends_with(".png"));
        assert_eq!(fs::read(dir.join(&name)).unwrap(), PNG);
        assert_eq!(keep_copy(&dir, &shot).as_deref(), Some(name.as_str()));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        // A copy cut short is written again.
        fs::write(dir.join(&name), &PNG[..4]).unwrap();
        assert_eq!(keep_copy(&dir, &shot).as_deref(), Some(name.as_str()));
        assert_eq!(fs::read(dir.join(&name)).unwrap(), PNG);
        let video = tmp.path().join("demo.mp4");
        fs::write(&video, b"\0\0\0\x18ftypmp42").unwrap();
        assert_eq!(keep_copy(&dir, &video), None);
        assert_eq!(keep_copy(&dir, &tmp.path().join("gone.png")), None);
    }

    #[test]
    fn images_older_than_thirty_days_are_removed() {
        let tmp = Temp::new("prune");
        let dir = tmp.path();
        let old = "0123456789abcdef0123456789abcdef.png";
        let new = "fedcba9876543210fedcba9876543210.png";
        fs::write(dir.join(old), PNG).unwrap();
        fs::write(dir.join(new), PNG).unwrap();
        fs::write(dir.join("kept.txt"), "x").unwrap();
        let part = format!(".{new}.part");
        fs::write(dir.join(&part), PNG).unwrap();
        let now = SystemTime::now();
        fs::File::options()
            .write(true)
            .open(dir.join(&part))
            .unwrap()
            .set_modified(now - PART_KEEP - Duration::from_secs(60))
            .unwrap();
        let month_ago = now - KEEP - Duration::from_secs(60);
        fs::File::options()
            .write(true)
            .open(dir.join(old))
            .unwrap()
            .set_modified(month_ago)
            .unwrap();
        prune(dir, now);
        assert!(!dir.join(old).exists());
        assert!(dir.join(new).exists());
        assert!(dir.join("kept.txt").exists());
        assert!(!dir.join(&part).exists());
    }
}
