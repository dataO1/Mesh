//! Live-mode startup: resolve the configured style (D35) or explain why not.
//!
//! A missing or invalid style never crashes the player. It becomes a
//! [`StyleProblem`] that [`LiveApp`](super::app::LiveApp) shows in place of
//! the performance screen, with the validation issues and the styles that
//! exist in the collection.

use std::path::Path;

use mesh_core::live::loader::LoadError;
use mesh_core::live::style::{self, Style, StyleError};

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
            vec!["Set `live.style: <name>` in player-config.yaml.".into()],
        ));
    };
    Style::load_named(collection, name).map_err(|e| match e {
        StyleError::InvalidName(n) => problem(format!("'{n}' is not a valid style name"), Vec::new()),
        StyleError::Io { path, source } => problem(
            format!("Style '{name}' not found"),
            vec![format!("Cannot read {}: {source}", path.display())],
        ),
        StyleError::Parse { path, source } => problem(
            format!("Style '{name}' cannot be parsed"),
            vec![format!("{}: {source}", path.display())],
        ),
        StyleError::Invalid { path, issues } => problem(
            format!("Style '{name}' is invalid ({} issue{})", issues.len(), if issues.len() == 1 { "" } else { "s" }),
            std::iter::once(format!("{}", path.display())).chain(issues.iter().map(|i| i.to_string())).collect(),
        ),
    })
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
}
