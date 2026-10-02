//! "Delete audio after": removes old recordings Fennec made, keeping the text.

use std::path::Path;
use std::time::{Duration, SystemTime};

use super::{Result, Store};
use crate::config::{Paths, Settings};

/// Deletes the audio of documents whose file in `audio_dir` was last written
/// more than `days` days before `now`, and clears their audio path. Files
/// outside `audio_dir` (imported originals) belong to the user and are left
/// alone. Returns how many files were deleted.
pub fn delete_old_audio(store: &Store, audio_dir: &Path, days: u32, now: SystemTime) -> Result<usize> {
    let max_age = Duration::from_secs(u64::from(days) * 24 * 60 * 60);
    let mut deleted = 0;
    for (doc, path) in store.documents_with_audio()? {
        if !path.starts_with(audio_dir) {
            continue;
        }
        let Ok(modified) = std::fs::metadata(&path).and_then(|m| m.modified()) else {
            continue;
        };
        if now.duration_since(modified).unwrap_or_default() <= max_age {
            continue;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => {
                store.set_audio_path(doc, None)?;
                deleted += 1;
            }
            Err(e) => tracing::warn!("could not delete old audio {}: {e}", path.display()),
        }
    }
    Ok(deleted)
}

/// Applies the user's "Delete audio after" setting on a worker thread, so
/// startup does not wait for it.
pub fn apply_in_background(paths: &Paths, settings: &Settings) -> Option<std::thread::JoinHandle<()>> {
    let days = settings.delete_audio_after_days?;
    let (db, audio) = (paths.database(), paths.audio());
    Some(std::thread::spawn(move || {
        match Store::open(&db).and_then(|s| delete_old_audio(&s, &audio, days, SystemTime::now())) {
            Ok(0) => {}
            Ok(n) => tracing::info!("deleted {n} audio files older than {days} days"),
            Err(e) => tracing::warn!("could not apply the audio retention setting: {e}"),
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::NewDocument;

    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

    fn doc_with_audio(store: &Store, path: &Path, age: Duration, now: SystemTime) -> i64 {
        let doc = store.create_document(&NewDocument::dictation("d")).unwrap();
        std::fs::write(path, b"RIFF").unwrap();
        let f = std::fs::File::options().write(true).open(path).unwrap();
        f.set_modified(now - age).unwrap();
        store.set_audio_path(doc, Some(path)).unwrap();
        doc
    }

    #[test]
    fn old_recordings_are_deleted_and_new_ones_kept() {
        let dir = tempfile::tempdir().unwrap();
        let audio = dir.path().join("audio");
        std::fs::create_dir_all(&audio).unwrap();
        let store = Store::open_in_memory().unwrap();
        let now = SystemTime::now();
        let old = doc_with_audio(&store, &audio.join("old.wav"), 31 * DAY, now);
        let new = doc_with_audio(&store, &audio.join("new.wav"), 29 * DAY, now);

        assert_eq!(delete_old_audio(&store, &audio, 30, now).unwrap(), 1);

        assert!(!audio.join("old.wav").exists());
        assert_eq!(store.document(old).unwrap().audio_path, None);
        assert!(audio.join("new.wav").exists());
        assert_eq!(
            store.document(new).unwrap().audio_path.as_deref(),
            Some(audio.join("new.wav").as_path())
        );
    }

    #[test]
    fn imported_originals_outside_the_audio_folder_are_never_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let audio = dir.path().join("audio");
        std::fs::create_dir_all(&audio).unwrap();
        let store = Store::open_in_memory().unwrap();
        let now = SystemTime::now();
        let original = dir.path().join("interview.mp3");
        let doc = doc_with_audio(&store, &original, 400 * DAY, now);

        assert_eq!(delete_old_audio(&store, &audio, 30, now).unwrap(), 0);
        assert!(original.exists());
        assert!(store.document(doc).unwrap().audio_path.is_some());
    }

    #[test]
    fn a_missing_file_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory().unwrap();
        let doc = store.create_document(&NewDocument::dictation("d")).unwrap();
        store
            .set_audio_path(doc, Some(&dir.path().join("gone.wav")))
            .unwrap();
        assert_eq!(
            delete_old_audio(&store, dir.path(), 1, SystemTime::now()).unwrap(),
            0
        );
    }

    #[test]
    fn never_means_nothing_runs() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        assert!(apply_in_background(&paths, &Settings::default()).is_none());
    }

    #[test]
    fn the_startup_hook_applies_the_setting() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        std::fs::create_dir_all(paths.audio()).unwrap();
        let file = paths.audio().join("doc1.wav");
        let doc = {
            let store = Store::open(&paths.database()).unwrap();
            doc_with_audio(&store, &file, 100 * DAY, SystemTime::now())
        };
        let settings = Settings {
            delete_audio_after_days: Some(90),
            ..Default::default()
        };
        apply_in_background(&paths, &settings).unwrap().join().unwrap();
        assert!(!file.exists());
        let store = Store::open(&paths.database()).unwrap();
        assert_eq!(store.document(doc).unwrap().audio_path, None);
    }
}
