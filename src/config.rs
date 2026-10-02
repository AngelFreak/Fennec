//! Where Fennec keeps things, and the user's settings
//! (`~/.config/fennec/settings.toml`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::commands::CommandTable;

#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
}

impl Paths {
    /// XDG locations for the current user.
    pub fn user() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        Self {
            config_dir: config.join("fennec"),
            data_dir: data.join("fennec"),
        }
    }

    /// Everything under one directory (tests).
    pub fn under(root: &Path) -> Self {
        Self {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
        }
    }

    pub fn database(&self) -> PathBuf {
        self.data_dir.join("fennec.db")
    }
    pub fn models(&self) -> PathBuf {
        self.data_dir.join("models")
    }
    pub fn audio(&self) -> PathBuf {
        self.data_dir.join("audio")
    }
    pub fn templates(&self) -> PathBuf {
        self.config_dir.join("templates")
    }
    pub fn prompts(&self) -> PathBuf {
        self.config_dir.join("prompts")
    }
    pub fn settings_file(&self) -> PathBuf {
        self.config_dir.join("settings.toml")
    }
    pub fn exports(&self) -> PathBuf {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        home.join("Documents").join("Fennec")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    #[default]
    Auto,
    Cuda,
    Vulkan,
    Cpu,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// GGML model file name inside the models directory, or an absolute path.
    pub model: String,
    pub backend: Backend,
    /// cpal device id; empty means the default microphone.
    pub microphone: String,
    /// Extra amplification of the microphone, in dB (-20 to +20).
    pub input_gain_db: f32,
    pub pause_ms: u32,
    pub show_preview: bool,
    pub keep_dictation_audio: bool,
    pub vocabulary: String,
    /// Used for `{user}` in templates.
    pub user_name: String,
    pub default_template: String,
    pub commands: CommandTable,
    pub ai: crate::ai::AiSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            model: "edda-v0.1-q5_0.bin".into(),
            backend: Backend::Auto,
            microphone: String::new(),
            input_gain_db: 0.0,
            pause_ms: 600,
            show_preview: true,
            keep_dictation_audio: true,
            vocabulary: String::new(),
            user_name: String::new(),
            default_template: "notat".into(),
            commands: CommandTable::default(),
            ai: crate::ai::AiSettings::default(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("{path}: {message}")]
    Invalid { path: PathBuf, message: String },
    #[error("could not write {path}: {source}")]
    Write { path: PathBuf, source: std::io::Error },
}

impl Settings {
    /// Missing file → defaults. A broken file is an error, so a typo does
    /// not silently reset the user's settings.
    pub fn load(path: &Path) -> Result<Self, SettingsError> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| SettingsError::Invalid {
                path: path.to_path_buf(),
                message: e.message().to_string(),
            }),
            Err(_) => Ok(Self::default()),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), SettingsError> {
        let write = |source| SettingsError::Write {
            path: path.to_path_buf(),
            source,
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(write)?;
        }
        let text = toml::to_string_pretty(self).expect("settings always serialize");
        std::fs::write(path, text).map_err(write)
    }

    pub fn model_path(&self, paths: &Paths) -> PathBuf {
        let p = PathBuf::from(&self.model);
        if p.is_absolute() {
            p
        } else {
            paths.models().join(p)
        }
    }

    pub fn vad_path(&self, paths: &Paths) -> PathBuf {
        paths.models().join(crate::models::VAD_FILE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_settings_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            Settings::load(&dir.path().join("none.toml")).unwrap(),
            Settings::default()
        );
    }

    #[test]
    fn settings_round_trip_and_unknown_keys_keep_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.toml");
        let s = Settings {
            user_name: "Ane".into(),
            pause_ms: 800,
            ..Default::default()
        };
        s.save(&p).unwrap();
        assert_eq!(Settings::load(&p).unwrap(), s);
        std::fs::write(&p, "user_name = \"Bo\"\n").unwrap();
        let partial = Settings::load(&p).unwrap();
        assert_eq!(partial.user_name, "Bo");
        assert_eq!(partial.pause_ms, 600);
    }

    #[test]
    fn a_broken_settings_file_is_an_error_naming_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.toml");
        std::fs::write(&p, "pause_ms = \"lang\"").unwrap();
        let err = Settings::load(&p).unwrap_err();
        assert!(err.to_string().contains("s.toml"), "{err}");
    }

    #[test]
    fn relative_model_names_live_in_the_models_directory() {
        let paths = Paths::under(Path::new("/r"));
        assert_eq!(
            Settings::default().model_path(&paths),
            Path::new("/r/data/models/edda-v0.1-q5_0.bin")
        );
        let abs = Settings {
            model: "/m/x.bin".into(),
            ..Default::default()
        };
        assert_eq!(abs.model_path(&paths), Path::new("/m/x.bin"));
    }
}
