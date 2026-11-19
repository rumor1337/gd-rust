//! Where the player's own Geometry Dash art is (RobTop's, never bundled).
//!
//! Order: `DEMO_ASSETS`, then the folder remembered from an earlier run,
//! then Steam's default install on Windows. Linux and macOS ask once with
//! a native folder picker and remember the answer.

use std::path::{Path, PathBuf};

/// The folder that holds GD's sprite sheets inside `dir`: the dir itself,
/// its Resources/ (a game install), or Contents/Resources (a macOS .app).
pub fn sheets_in(dir: &Path) -> Option<PathBuf> {
    [dir.to_path_buf(), dir.join("Resources"), dir.join("Contents/Resources")]
        .into_iter()
        .find(|d| d.join("GJ_GameSheet-uhd.plist").exists())
}

/// The remembered folder lives here (one line: the path).
fn saved_file() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        home?.join("Library/Application Support")
    } else if cfg!(windows) {
        PathBuf::from(std::env::var_os("APPDATA")?)
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| home.map(|h| h.join(".config")))?
    };
    Some(base.join("gd-rust").join("assets_dir"))
}

/// The art folder without asking: env, remembered, or (Windows) Steam's default.
pub fn find() -> Option<PathBuf> {
    let env = std::env::var_os("DEMO_ASSETS").map(PathBuf::from);
    let saved = saved_file().and_then(|f| std::fs::read_to_string(f).ok()).map(|s| PathBuf::from(s.trim()));
    let steam = cfg!(windows).then(|| PathBuf::from(r"C:\Program Files (x86)\Steam\steamapps\common\Geometry Dash"));
    [env, saved, steam].into_iter().flatten().find_map(|d| sheets_in(&d))
}

/// The art folder, asking with a native folder picker when none is known
/// yet (Linux / macOS; remembered for later runs). Exits if the player cancels.
pub fn find_or_ask() -> PathBuf {
    if let Some(d) = find() {
        return d;
    }
    #[cfg(not(windows))]
    loop {
        let picked = rfd::FileDialog::new()
            .set_title("Choose your Geometry Dash folder (the game's art is read from it)")
            .pick_folder();
        let Some(dir) = picked else { break };
        if let Some(sheets) = sheets_in(&dir) {
            if let Some(f) = saved_file() {
                let saved = f.parent().map(std::fs::create_dir_all).transpose().and_then(|_| std::fs::write(&f, sheets.to_string_lossy().as_bytes()));
                if let Err(e) = saved {
                    eprintln!("could not remember the folder ({}): {e}", f.display());
                }
            }
            return sheets;
        }
        rfd::MessageDialog::new()
            .set_title("Not a Geometry Dash folder")
            .set_description(format!("{} has no GJ_GameSheet-uhd.plist. Pick the folder Geometry Dash is installed in (or its Resources folder).", dir.display()))
            .show();
    }
    eprintln!("no Geometry Dash art folder: install Geometry Dash, or set DEMO_ASSETS to its folder");
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_sheets_in_an_install_a_resources_dir_or_an_app() {
        let root = std::env::temp_dir().join(format!("gdrust-assets-{}", std::process::id()));
        for sub in ["game/Resources", "GD.app/Contents/Resources", "plain"] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
            std::fs::write(root.join(sub).join("GJ_GameSheet-uhd.plist"), "").unwrap();
        }
        assert_eq!(sheets_in(&root.join("game")), Some(root.join("game/Resources")));
        assert_eq!(sheets_in(&root.join("GD.app")), Some(root.join("GD.app/Contents/Resources")));
        assert_eq!(sheets_in(&root.join("plain")), Some(root.join("plain")));
        assert_eq!(sheets_in(&root), None);
        let _ = std::fs::remove_dir_all(root);
    }
}
