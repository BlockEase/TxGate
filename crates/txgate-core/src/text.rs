//! Translatable messages. The engine sends a key and named parameters with
//! every user-facing message; the UI translates it at display time. English
//! templates live in the UI's `en.json`, so both sides render the same text.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

type Dict = BTreeMap<String, String>;

fn parse(name: &str, json: &str) -> Dict {
    serde_json::from_str(json).unwrap_or_else(|e| panic!("{name}.json is not a flat JSON object: {e}"))
}

static ENGLISH: LazyLock<Dict> = LazyLock::new(|| parse("en", include_str!("../../../ui/src/i18n/locales/en.json")));

/// Translations shared with the UI, by language code.
static LOCALES: LazyLock<Vec<(&'static str, Dict)>> = LazyLock::new(|| {
    vec![
        ("zh-CN", parse("zh-CN", include_str!("../../../ui/src/i18n/locales/zh-CN.json"))),
        ("es", parse("es", include_str!("../../../ui/src/i18n/locales/es.json"))),
        ("ru", parse("ru", include_str!("../../../ui/src/i18n/locales/ru.json"))),
        ("pt", parse("pt", include_str!("../../../ui/src/i18n/locales/pt.json"))),
    ]
});

/// The template for `key` in `lang`, falling back to English.
fn template(lang: &str, key: &str) -> Option<&'static str> {
    LOCALES
        .iter()
        .find(|(code, _)| *code == lang)
        .and_then(|(_, dict)| dict.get(key))
        .or_else(|| ENGLISH.get(key))
        .map(String::as_str)
}

/// A plain UI string (no parameters) in `lang`, e.g. for notifications.
pub fn tr(lang: &str, key: &str, params: &[(&str, String)]) -> String {
    let params: BTreeMap<String, String> = params.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
    render(template(lang, key).unwrap_or(key), &params)
}

/// A message as a translation key plus named `{param}` values.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Text {
    pub key: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
    /// Texts shown before this one, such as "Call 2: ".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prefix: Vec<Text>,
}

impl Text {
    pub fn new<K: Into<String>, V: Into<String>>(key: &str, params: impl IntoIterator<Item = (K, V)>) -> Self {
        Self { key: key.into(), params: params.into_iter().map(|(k, v)| (k.into(), v.into())).collect(), prefix: Vec::new() }
    }

    /// A text without parameters.
    pub fn plain(key: &str) -> Self {
        Self::new::<&str, &str>(key, [])
    }

    /// The English rendering.
    pub fn english(&self) -> String {
        assert!(ENGLISH.contains_key(&self.key), "missing English text for {}", self.key);
        self.localized("en")
    }

    /// The rendering in `lang` (a code such as `zh-CN`), falling back to English.
    pub fn localized(&self, lang: &str) -> String {
        let prefix: String = self.prefix.iter().map(|p| p.localized(lang)).collect();
        prefix + &render(template(lang, &self.key).unwrap_or(&self.key), &self.params)
    }
}

/// Replaces every `{name}` in `template` with its parameter.
pub fn render(template: &str, params: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        match rest[start..].find('}') {
            Some(len) => {
                let name = &rest[start + 1..start + len];
                match params.get(name) {
                    Some(v) => out.push_str(v),
                    None => out.push_str(&rest[start..=start + len]),
                }
                rest = &rest[start + len + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placeholders(s: &str) -> std::collections::BTreeSet<&str> {
        s.split('{').skip(1).filter_map(|p| p.split_once('}').map(|(name, _)| name)).collect()
    }

    #[test]
    fn every_language_translates_every_text_with_the_same_placeholders() {
        let mut problems = Vec::new();
        for (lang, dict) in LOCALES.iter() {
            for (key, en) in ENGLISH.iter() {
                match dict.get(key) {
                    None => problems.push(format!("{lang}: missing {key}")),
                    Some(tr) if placeholders(tr) != placeholders(en) => problems.push(format!("{lang}: {key} has placeholders {:?}, English {:?}", placeholders(tr), placeholders(en))),
                    Some(_) => {}
                }
            }
            for key in dict.keys().filter(|k| !ENGLISH.contains_key(*k)) {
                problems.push(format!("{lang}: unknown key {key}"));
            }
        }
        assert!(problems.is_empty(), "{} problems:\n{}", problems.len(), problems.join("\n"));
    }

    #[test]
    fn renders_in_the_chosen_language_and_falls_back_to_english() {
        let t = Text::new("risk.unlimited_approval", [("spender", "0xAbC…1234")]);
        assert_eq!(t.localized("zh-CN"), "无限额度授权给 0xAbC…1234：它随时可以转走此代币的全部余额");
        assert_eq!(t.localized("xx"), t.english());
    }
}
