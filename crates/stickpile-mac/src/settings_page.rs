//! The Settings tab: the editable keys of `config.toml` (the GTK page's
//! rows, `stickpile_core::settings::settings_schema`), with their values
//! and whether a read-only `config.toml` locks them.

use std::path::{Path, PathBuf};

use stickpile_core::settings::{self, LayeredConfig, GROUPS};
use stickpile_desktop::pages;
use stickpile_i18n::global::t_dyn;

use crate::bridge::{ChoiceOption, SettingKind, SettingRow, SettingValue, SettingsGroup, SettingsView};

#[derive(Debug, Default)]
pub struct SettingsPage {
    layered: Option<LayeredConfig>,
    error: Option<String>,
    save_error: Option<String>,
}

impl SettingsPage {
    pub fn show(&mut self, layered: LayeredConfig) {
        self.layered = Some(layered);
        self.error = None;
    }

    pub fn set_error(&mut self, error: Option<String>) {
        self.error = error;
    }

    pub fn save_failed(&mut self, error: Option<String>) {
        self.save_error = error;
    }

    /// Where edits are written: `config.toml`, or `settings.toml` beside a
    /// read-only one.
    pub fn write_path(&self, config_path: &Path) -> PathBuf {
        match &self.layered {
            Some(l) => l.write_path.clone(),
            None => config_path.to_path_buf(),
        }
    }

    pub fn view(&self, config_path: &Path) -> SettingsView {
        let Some(layered) = &self.layered else {
            return SettingsView {
                error: self.error.clone(),
                note: String::new(),
                path: config_path.display().to_string(),
                warnings: Vec::new(),
                groups: Vec::new(),
                save_error: self.save_error.clone(),
            };
        };
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let reason = pages::lock_reason(config_path);
        let schema = settings::settings_schema();
        let groups = GROUPS
            .iter()
            .map(|group| SettingsGroup {
                title: t_dyn(group),
                rows: schema
                    .iter()
                    .filter(|spec| spec.group == *group)
                    .map(|spec| {
                        let value = settings::get_value(&layered.config, spec.key);
                        let locked = layered.locked.contains(spec.key);
                        let help = t_dyn(spec.help);
                        SettingRow {
                            key: spec.key.to_string(),
                            label: t_dyn(spec.label),
                            help: if locked { format!("{help} {reason}.") } else { help },
                            kind: match &spec.kind {
                                settings::SettingKind::Bool => SettingKind::Bool,
                                settings::SettingKind::Int { min, max } => SettingKind::Int { min: *min, max: *max },
                                settings::SettingKind::Choice(names) => SettingKind::Choice {
                                    options: names
                                        .iter()
                                        .map(|n| ChoiceOption {
                                            value: n.to_string(),
                                            label: pages::choice_label(spec.key, n),
                                        })
                                        .collect(),
                                },
                                settings::SettingKind::Text => SettingKind::Text,
                            },
                            value: value
                                .as_ref()
                                .and_then(from_toml)
                                .or_else(|| from_toml(&spec.default))
                                .unwrap_or(SettingValue::Text { value: String::new() }),
                            locked,
                            resettable: !locked && value.as_ref() != Some(&spec.default),
                        }
                    })
                    .collect(),
            })
            .filter(|g| !g.rows.is_empty())
            .collect();
        SettingsView {
            error: self.error.clone(),
            note: pages::save_note(config_path, &layered.write_path, home.as_deref()),
            path: layered.write_path.display().to_string(),
            warnings: layered.warnings.clone(),
            groups,
            save_error: self.save_error.clone(),
        }
    }
}

pub fn from_toml(v: &toml::Value) -> Option<SettingValue> {
    Some(match v {
        toml::Value::Boolean(b) => SettingValue::Bool { value: *b },
        toml::Value::Integer(i) => SettingValue::Int { value: *i },
        toml::Value::String(s) => SettingValue::Text { value: s.clone() },
        _ => return None,
    })
}

pub fn to_toml(v: SettingValue) -> toml::Value {
    match v {
        SettingValue::Bool { value } => toml::Value::Boolean(value),
        SettingValue::Int { value } => toml::Value::Integer(value),
        SettingValue::Text { value } => toml::Value::String(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_follow_the_schema() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.toml");
        std::fs::write(&config, "[ui]\ncolumns = 4\n").unwrap();
        let mut page = SettingsPage::default();
        page.show(settings::load_layered(&config).unwrap());
        let v = page.view(&config);
        assert!(v.error.is_none());
        let rows: Vec<&SettingRow> = v.groups.iter().flat_map(|g| &g.rows).collect();
        assert_eq!(rows.len(), settings::settings_schema().len());
        let columns = rows.iter().find(|r| r.key == "ui.columns").unwrap();
        assert_eq!(columns.value, SettingValue::Int { value: 4 });
        assert!(columns.resettable && !columns.locked);
        let modal = rows.iter().find(|r| r.key == "ui.modal").unwrap();
        assert_eq!(modal.value, SettingValue::Bool { value: true });
        assert!(!modal.resettable);
        assert!(rows
            .iter()
            .any(|r| matches!(&r.kind, SettingKind::Choice { options } if !options.is_empty())));
    }
}
