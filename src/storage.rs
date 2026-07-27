use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
};

use thiserror::Error;
use uuid::Uuid;

use crate::model::{Deck, ValidationIssue};

#[derive(Debug, Error)]
pub enum DeckFileError {
    #[error("Could not read the file: {0}")]
    Read(#[source] io::Error),
    #[error("The file is not valid JSON: {0}")]
    Json(#[source] serde_json::Error),
    #[error("The deck contains {0} validation issue(s)")]
    Validation(usize, Vec<ValidationIssue>),
    #[error("Could not write the file: {0}")]
    Write(#[source] io::Error),
}

impl DeckFileError {
    pub fn issues(&self) -> Option<&[ValidationIssue]> {
        match self {
            Self::Validation(_, issues) => Some(issues),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum LibraryEntry {
    Valid {
        path: PathBuf,
        deck: Deck,
    },
    Invalid {
        path: PathBuf,
        title: String,
        error: String,
        issues: Vec<ValidationIssue>,
    },
}

impl LibraryEntry {
    pub fn path(&self) -> &Path {
        match self {
            Self::Valid { path, .. } | Self::Invalid { path, .. } => path,
        }
    }
}

pub fn load_deck(path: &Path) -> Result<Deck, DeckFileError> {
    let contents = fs::read_to_string(path).map_err(DeckFileError::Read)?;
    let deck: Deck = serde_json::from_str(&contents).map_err(DeckFileError::Json)?;
    let issues = deck.validate();
    if issues.is_empty() {
        Ok(deck)
    } else {
        Err(DeckFileError::Validation(issues.len(), issues))
    }
}

pub fn scan_library(path: &Path) -> io::Result<Vec<LibraryEntry>> {
    fs::create_dir_all(path)?;
    let mut entries = Vec::new();
    for item in fs::read_dir(path)? {
        let path = item?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        match load_deck(&path) {
            Ok(deck) => entries.push(LibraryEntry::Valid { path, deck }),
            Err(error) => {
                let issues = error.issues().unwrap_or_default().to_vec();
                let title = path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .unwrap_or("Invalid deck")
                    .to_owned();
                entries.push(LibraryEntry::Invalid {
                    path,
                    title,
                    error: error.to_string(),
                    issues,
                });
            }
        }
    }
    entries.sort_by_key(|entry| {
        entry
            .path()
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    });
    Ok(entries)
}

pub fn save_deck(path: &Path, deck: &Deck) -> Result<(), DeckFileError> {
    let issues = deck.validate();
    if !issues.is_empty() {
        return Err(DeckFileError::Validation(issues.len(), issues));
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(DeckFileError::Write)?;
    let temporary = parent.join(format!(".flashcards-{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut file = File::create(&temporary)?;
        serde_json::to_writer_pretty(&mut file, deck).map_err(io::Error::other)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
        Ok::<_, io::Error>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(DeckFileError::Write)
}

pub fn unique_deck_path(library: &Path, title: &str) -> PathBuf {
    let stem = filename_stem(title);
    let first = library.join(format!("{stem}.json"));
    if !first.exists() {
        return first;
    }
    for number in 2.. {
        let candidate = library.join(format!("{stem}-{number}.json"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

pub fn import_deck(source: &Path, library: &Path) -> Result<PathBuf, DeckFileError> {
    let mut deck = load_deck(source)?;
    if scan_library(library)
        .map_err(DeckFileError::Write)?
        .iter()
        .any(|entry| matches!(entry, LibraryEntry::Valid { deck: existing, .. } if existing.id == deck.id))
    {
        deck.id = Uuid::new_v4();
    }
    let destination = unique_deck_path(library, &deck.title);
    save_deck(&destination, &deck)?;
    Ok(destination)
}

fn filename_stem(title: &str) -> String {
    let stem: String = title
        .trim()
        .to_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect();
    let stem = stem
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if stem.is_empty() {
        "deck".to_owned()
    } else {
        stem
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Card;

    #[test]
    fn round_trip_is_pretty_and_valid() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("deck.json");
        let mut deck = Deck::new("Example");
        deck.cards.push(Card::new("Front", "Back"));
        save_deck(&path, &deck).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.ends_with('\n'));
        assert!(text.contains("  \"schema_version\""));
        assert_eq!(load_deck(&path).unwrap(), deck);
    }

    #[test]
    fn imports_use_unique_names_and_ids() {
        let source_directory = tempfile::tempdir().unwrap();
        let library = tempfile::tempdir().unwrap();
        let source = source_directory.path().join("source.json");
        let deck = Deck::new("My Deck");
        save_deck(&source, &deck).unwrap();

        let first = import_deck(&source, library.path()).unwrap();
        let second = import_deck(&source, library.path()).unwrap();
        assert_eq!(first.file_name().unwrap(), "my-deck.json");
        assert_eq!(second.file_name().unwrap(), "my-deck-2.json");
        assert_ne!(
            load_deck(&first).unwrap().id,
            load_deck(&second).unwrap().id
        );
    }

    #[test]
    fn invalid_json_remains_discoverable() {
        let library = tempfile::tempdir().unwrap();
        fs::write(library.path().join("broken.json"), "{ nope").unwrap();
        assert!(matches!(
            scan_library(library.path()).unwrap().as_slice(),
            [LibraryEntry::Invalid { .. }]
        ));
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("future.json");
        let deck = Deck::new("Deck");
        let mut value = serde_json::to_value(deck).unwrap();
        value["surprise"] = serde_json::json!(true);
        fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        assert!(matches!(load_deck(&path), Err(DeckFileError::Json(_))));
    }
}
