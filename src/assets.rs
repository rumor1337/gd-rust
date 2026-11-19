//! Where the player's own Geometry Dash art is (RobTop's, never bundled).
//!
//! Order: `--assets <dir>`, `DEMO_ASSETS`, the folder remembered from an
//! earlier run, Steam's default install (Windows); else a native folder
//! picker asks once and the answer is remembered.

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

/// `--assets <dir>` taken out of the command line: (the dir, the other args).
pub fn split_flag(mut args: Vec<String>) -> (Option<PathBuf>, Vec<String>) {
    let Some(i) = args.iter().position(|a| a == "--assets") else { return (None, args) };
    args.remove(i);
    let dir = (i < args.len()).then(|| PathBuf::from(args.remove(i)));
    (dir, args)
}

/// The art folder, asking with a native folder picker when none is known
/// yet (remembered for later runs). Exits if the player cancels.
pub fn find_or_ask() -> PathBuf {
    if let (Some(dir), _) = split_flag(std::env::args().collect()) {
        return sheets_in(&dir).unwrap_or_else(|| {
            eprintln!("--assets {}: no GJ_GameSheet-uhd.plist there (give the Geometry Dash folder)", dir.display());
            std::process::exit(1);
        });
    }
    if let Some(d) = find() {
        return d;
    }
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

    #[test]
    fn assets_flag_is_taken_out_of_the_args() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(split_flag(a(&["demo", "--assets", "C:\\GD", "11"])), (Some(PathBuf::from("C:\\GD")), a(&["demo", "11"])));
        assert_eq!(split_flag(a(&["demo", "11"])), (None, a(&["demo", "11"])));
        assert_eq!(split_flag(a(&["demo", "--assets"])), (None, a(&["demo"])));
    }
}
