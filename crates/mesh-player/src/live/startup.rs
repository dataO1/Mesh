//! Live-mode startup: resolve the configured style (D35) or explain why not.
//!
//! A missing or invalid style never crashes the player. It becomes a
//! [`StyleProblem`] that [`LiveApp`](super::app::LiveApp) shows in place of
//! the performance screen, with the validation issues and the styles that
//! exist in the collection.
//!
//! The style comes from `live.style` in `player-config.yaml`, or from the
//! `--live [STYLE]` command-line flag ([`live_flag`]), which forces live mode
//! for one run without touching the config file. `STYLE` is a style name
//! under `<collection>/styles/` or a path to a style folder.

use std::path::{Path, PathBuf};

use mesh_core::live::loader::LoadError;
use mesh_core::live::style::{self, Style, StyleError};

/// Command-line flag that forces live mode for this run.
pub const LIVE_FLAG: &str = "--live";

/// Where the style comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StyleSource {
    /// Folder name under `<collection>/styles/`.
    Named(String),
    /// A style folder anywhere on disk.
    Path(PathBuf),
}

impl StyleSource {
    /// A value with a path separator (or `.` / `..`) is a path, anything
    /// else a style name.
    pub fn parse(s: &str) -> Self {
        let s = s.trim();
        if s.contains('/') || s.contains('\\') || s == "." || s == ".." {
            Self::Path(PathBuf::from(s))
        } else {
            Self::Named(s.to_string())
        }
    }
}

/// The `--live [STYLE]` flag in `args` (without the program name).
///
/// - `None`: flag absent, run mode comes from the config.
/// - `Some(None)`: `--live` alone, style comes from `live.style`.
/// - `Some(Some(source))`: `--live STYLE` or `--live=STYLE`.
///
/// The value after `--live` is taken as the style unless it is another
/// flag (starts with `--`).
pub fn live_flag(args: &[String]) -> Option<Option<StyleSource>> {
    let mut found = None;
    let mut it = args.iter().peekable();
    while let Some(arg) = it.next() {
        if arg == LIVE_FLAG {
            let value = it.next_if(|next| !next.starts_with("--"));
            found = Some(value.map(|v| v.as_str()));
        } else if let Some(value) = arg.strip_prefix("--live=") {
            found = Some(Some(value));
        }
    }
    found.map(|v| v.map(str::trim).filter(|v| !v.is_empty()).map(StyleSource::parse))
}

/// Collection that receives captures and recordings for a style loaded from
/// `dir`. A style inside some `<root>/styles/<name>` folder uses `<root>`, so
/// a development style kept outside the real collection keeps its captures
/// next to it. Any other folder falls back to `fallback`.
pub fn collection_for_style_dir(dir: &Path, fallback: &Path) -> PathBuf {
    let parent = dir.parent().filter(|p| p.file_name().is_some_and(|n| n == style::STYLES_FOLDER));
    match parent.and_then(Path::parent) {
        Some(root) if !root.as_os_str().is_empty() => root.to_path_buf(),
        _ => fallback.to_path_buf(),
    }
}

/// Why live mode cannot start, ready for display.
#[derive(Debug, Clone, PartialEq)]
pub struct StyleProblem {
    pub title: String,
    pub details: Vec<String>,
    /// Style folders under `<collection>/styles/` that hold a `style.yaml`.
    pub available: Vec<String>,
}

/// Names of the style folders in `collection`, sorted.
pub fn available_styles(collection: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(style::styles_dir(collection)) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().join(style::STYLE_FILE).is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

/// Load and validate the style named by `live.style`.
pub fn resolve_style(collection: &Path, name: Option<&str>) -> Result<Style, StyleProblem> {
    let problem = |title: String, details: Vec<String>| StyleProblem {
        title,
        details,
        available: available_styles(collection),
    };
    let Some(name) = name.map(str::trim).filter(|n| !n.is_empty()) else {
        return Err(problem(
            "No live style configured".into(),
            vec!["Set `live.style: <name>` in player-config.yaml or pass `--live <style>`.".into()],
        ));
    };
    Style::load_named(collection, name).map_err(|e| style_error_problem(name, e, available_styles(collection)))
}

/// Load and validate the style folder `dir` (`--live <path>`). `collection`
/// only supplies the list of available styles for the error screen.
pub fn resolve_style_dir(dir: &Path, collection: &Path) -> Result<Style, StyleProblem> {
    let label = dir.display().to_string();
    Style::load(dir).map_err(|e| style_error_problem(&label, e, available_styles(collection)))
}

fn style_error_problem(name: &str, e: StyleError, available: Vec<String>) -> StyleProblem {
    let (title, details) = match e {
        StyleError::InvalidName(n) => (format!("'{n}' is not a valid style name"), Vec::new()),
        StyleError::Io { path, source } => {
            (format!("Style '{name}' not found"), vec![format!("Cannot read {}: {source}", path.display())])
        }
        StyleError::Parse { path, source } => {
            (format!("Style '{name}' cannot be parsed"), vec![format!("{}: {source}", path.display())])
        }
        StyleError::Invalid { path, issues } => (
            format!("Style '{name}' is invalid ({} issue{})", issues.len(), if issues.len() == 1 { "" } else { "s" }),
            std::iter::once(format!("{}", path.display())).chain(issues.iter().map(|i| i.to_string())).collect(),
        ),
    };
    StyleProblem { title, details, available }
}

/// A valid style whose assets could not be loaded into RAM.
pub fn load_problem(collection: &Path, name: &str, err: &LoadError) -> StyleProblem {
    StyleProblem {
        title: format!("Style '{name}' failed to load"),
        details: vec![err.to_string()],
        available: available_styles(collection),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../mesh-core/tests/fixtures/live-style")
    }

    /// Temp collection holding a copy of the fixture style as `breaks`.
    fn collection() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        copy_dir(&fixture(), &style::style_dir(dir.path(), "breaks"));
        dir
    }

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let target = to.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                copy_dir(&e.path(), &target);
            } else {
                std::fs::copy(e.path(), target).unwrap();
            }
        }
    }

    #[test]
    fn valid_style_resolves() {
        let dir = collection();
        let style = resolve_style(dir.path(), Some("breaks")).expect("fixture style is valid");
        assert_eq!(style.spec.lanes.len(), 4);
    }

    #[test]
    fn missing_or_unset_style_lists_available() {
        let dir = collection();
        let p = resolve_style(dir.path(), None).unwrap_err();
        assert_eq!(p.title, "No live style configured");
        assert_eq!(p.available, vec!["breaks".to_string()]);

        let p = resolve_style(dir.path(), Some("  ")).unwrap_err();
        assert_eq!(p.title, "No live style configured");

        let p = resolve_style(dir.path(), Some("jungle")).unwrap_err();
        assert_eq!(p.title, "Style 'jungle' not found");
        assert_eq!(p.available, vec!["breaks".to_string()]);

        let p = resolve_style(dir.path(), Some("../x")).unwrap_err();
        assert!(p.title.contains("not a valid style name"));
    }

    #[test]
    fn invalid_style_lists_each_issue() {
        let dir = collection();
        let yaml = style::style_dir(dir.path(), "breaks").join(style::STYLE_FILE);
        let text = std::fs::read_to_string(&yaml).unwrap().replace("tempo: { default: 172", "tempo: { default: 400");
        std::fs::write(&yaml, text).unwrap();
        let p = resolve_style(dir.path(), Some("breaks")).unwrap_err();
        assert!(p.title.starts_with("Style 'breaks' is invalid"), "{}", p.title);
        assert!(p.details.iter().any(|d| d.starts_with("tempo")), "{:?}", p.details);

        std::fs::write(&yaml, "name: [").unwrap();
        let p = resolve_style(dir.path(), Some("breaks")).unwrap_err();
        assert_eq!(p.title, "Style 'breaks' cannot be parsed");
    }

    #[test]
    fn empty_collection_has_no_styles() {
        let dir = tempfile::tempdir().unwrap();
        assert!(available_styles(dir.path()).is_empty());
    }

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn live_flag_parsing() {
        assert_eq!(live_flag(&args(&[])), None);
        assert_eq!(live_flag(&args(&["--midi-learn"])), None);
        assert_eq!(live_flag(&args(&["--live"])), Some(None));
        assert_eq!(live_flag(&args(&["--live", "--midi-learn"])), Some(None));
        assert_eq!(live_flag(&args(&["--live", "breaks"])), Some(Some(StyleSource::Named("breaks".into()))));
        assert_eq!(live_flag(&args(&["--live=breaks"])), Some(Some(StyleSource::Named("breaks".into()))));
        assert_eq!(live_flag(&args(&["--live="])), Some(None));
        assert_eq!(
            live_flag(&args(&["--midi-learn", "--live", "/x/styles/starter"])),
            Some(Some(StyleSource::Path("/x/styles/starter".into())))
        );
        assert_eq!(live_flag(&args(&["--live", "./starter"])), Some(Some(StyleSource::Path("./starter".into()))));
        assert_eq!(live_flag(&args(&["--live", "."])), Some(Some(StyleSource::Path(".".into()))));
    }

    #[test]
    fn captures_follow_a_styles_folder() {
        let fallback = Path::new("/home/u/Music/mesh-collection");
        assert_eq!(
            collection_for_style_dir(Path::new("/home/u/Music/dev/styles/starter"), fallback),
            PathBuf::from("/home/u/Music/dev")
        );
        assert_eq!(collection_for_style_dir(Path::new("/tmp/starter"), fallback), fallback);
        assert_eq!(collection_for_style_dir(Path::new("styles/starter"), fallback), fallback);
    }

    #[test]
    fn style_dir_resolves_outside_the_collection() {
        let dir = collection();
        let empty = tempfile::tempdir().unwrap();
        let style = resolve_style_dir(&style::style_dir(dir.path(), "breaks"), empty.path()).expect("fixture style is valid");
        assert_eq!(style.spec.lanes.len(), 4);

        let p = resolve_style_dir(&empty.path().join("nope"), dir.path()).unwrap_err();
        assert!(p.title.contains("not found"), "{}", p.title);
        assert_eq!(p.available, vec!["breaks".to_string()]);
    }
}
