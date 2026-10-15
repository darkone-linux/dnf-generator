//! What the installer offers: the locales of the framework, each with its
//! keyboard (`dnf/config/locales.json`, also read by `lib/i18n.nix`), and the
//! timezones of their country (`zone.tab` of tzdata).

use std::fs;
use std::path::Path;

use serde::Deserialize;

use super::Result;

pub const ZONE_TAB: &str = "/etc/zoneinfo/zone.tab";

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocaleEntry {
    pub locale: String,
    pub label: String,

    /// kbd keymap, what `loadkeys` takes.
    pub console: String,
    pub xkb: String,
    #[serde(default)]
    pub xkb_variant: String,
}

impl LocaleEntry {
    /// `fr_FR.UTF-8` → `FR`.
    pub fn country(&self) -> &str {
        self.locale.get(3..5).unwrap_or_default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct Catalog {
    pub locales: Vec<LocaleEntry>,

    /// Content of `zone.tab`; empty when tzdata is missing.
    pub zone_tab: String,
}

impl Catalog {
    pub fn load(dnf_source: &Path) -> Result<Self> {
        let path = dnf_source.join("config/locales.json");
        let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self {
            locales: serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?,
            zone_tab: fs::read_to_string(ZONE_TAB).unwrap_or_default(),
        })
    }

    pub fn locale(&self, locale: &str) -> Option<&LocaleEntry> {
        self.locales.iter().find(|l| l.locale == locale)
    }

    /// Timezones of a country, main one first (`zone.tab` order).
    pub fn timezones(&self, country: &str) -> Vec<String> {
        self.zone_tab
            .lines()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| {
                let mut cols = l.split('\t');
                let code = cols.next()?;
                let zone = cols.nth(1)?;
                (code == country).then(|| zone.to_string())
            })
            .collect()
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn catalog() -> Catalog {
        Catalog {
            locales: serde_json::from_str(
                r#"[
                  { "locale": "en_GB.UTF-8", "label": "English (United Kingdom)", "console": "uk", "xkb": "gb" },
                  { "locale": "fr_FR.UTF-8", "label": "Français (France)", "console": "fr", "xkb": "fr", "xkbVariant": "oss" },
                  { "locale": "es_ES.UTF-8", "label": "Español (España)", "console": "es", "xkb": "es" }
                ]"#,
            )
            .unwrap(),
            zone_tab: "# comment\nES\t+4024-00341\tEurope/Madrid\tSpain (mainland)\n\
                       ES\t+3553-00519\tAfrica/Ceuta\tCeuta, Melilla\n\
                       FR\t+4852+00220\tEurope/Paris\n\
                       GB\t+513030-0000731\tEurope/London\n"
                .to_string(),
        }
    }

    #[test]
    fn timezones_keep_the_main_one_first() {
        let c = catalog();
        assert_eq!(c.timezones("ES"), ["Europe/Madrid", "Africa/Ceuta"]);
        assert_eq!(c.timezones("FR"), ["Europe/Paris"]);
        assert!(c.timezones("US").is_empty());
    }

    #[test]
    fn locale_entry_reads_the_framework_table() {
        let c = catalog();
        let fr = c.locale("fr_FR.UTF-8").unwrap();
        assert_eq!((fr.country(), fr.xkb_variant.as_str()), ("FR", "oss"));
        assert_eq!(c.locale("en_GB.UTF-8").unwrap().console, "uk");
        assert!(c.locale("ru_RU.UTF-8").is_none());
    }
}
