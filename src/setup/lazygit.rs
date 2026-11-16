//! The `lazygit` part: takes out the lazygit config earlier builds (0.7.0
//! to 0.9.0) offered to write where you had none, holding only
//! `quitOnTopLevelReturn: true`. Written into lazygit's own config, it
//! changed lazygit everywhere; lazygit in Switchboard's panes now gets that
//! setting from a file of Switchboard's own, which each pane's
//! `LG_CONFIG_FILE` names ahead of your config (src/diff/lazygit.rs), and
//! lazygit elsewhere is left as you set it up.
//!
//! Only a file still exactly as it was written goes, wherever lazygit reads
//! one from: one you have changed is yours, and kept, and so is a link,
//! which is a file of yours put there. The guided setup asks first and says
//! nothing where there is none; a new build's first run never takes it out
//! by itself.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

use super::{Report, home_tilde};
use crate::diff::lazygit;
use crate::paths;

/// What earlier builds wrote, byte for byte.
const WRITTEN: &str = "# yaml-language-server: $schema=https://raw.githubusercontent.com/jesseduffield/lazygit/master/schema/config.json
# Written by `switchboard setup lazygit` where you had no lazygit config; it is
# yours now, and setup never writes over it.

# Esc with nothing left to back out of quits, as q and Ctrl-C do, so lazygit
# in a pane's place (M-p's \"Open lazygit in place of this pane\") closes the
# way the diff and the views do, bringing the pane back.
quitOnTopLevelReturn: true
";

/// Whether `file` is the config an earlier build wrote, untouched since.
fn as_written(file: &std::path::Path) -> bool {
    fs::symlink_metadata(file).is_ok_and(|m| m.is_file())
        && fs::read_to_string(file).is_ok_and(|t| t == WRITTEN)
}

/// The configs an earlier build wrote, still as it wrote them.
pub fn written() -> Vec<PathBuf> {
    lazygit::places(&lazygit::env, &paths::home(), lazygit::mac())
        .into_iter()
        .filter(|f| as_written(f))
        .collect()
}

/// The part.
pub fn run(r: &mut Report) -> Result<()> {
    for file in written() {
        if !r.check {
            fs::remove_file(&file)
                .with_context(|| format!("can't remove {}", home_tilde(&file)))?;
        }
        r.change(format!(
            "take out {}, the lazygit config an earlier version wrote, which lazygit in Switchboard's panes no longer needs",
            home_tilde(&file)
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::socket::testing::TempDir;

    #[test]
    fn only_the_file_as_it_was_written_is_taken_for_it() {
        let dir = TempDir::new();
        let file = dir.path().join("config.yml");
        fs::write(&file, WRITTEN).unwrap();
        assert!(as_written(&file));
        fs::write(&file, format!("{WRITTEN}gui:\n  sidePanelWidth: 0.2\n")).unwrap();
        assert!(!as_written(&file));
        let link = dir.path().join("link.yml");
        fs::write(&file, WRITTEN).unwrap();
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(!as_written(&link));
        assert!(!as_written(&dir.path().join("gone.yml")));
        // Switchboard's own config for the lazygit it opens is another file.
        assert_ne!(lazygit::ASSET, WRITTEN);
    }
}
