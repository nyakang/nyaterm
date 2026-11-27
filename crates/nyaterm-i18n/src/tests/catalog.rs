//! Strict source validation before serde_json or rust-i18n can overwrite keys.

use std::collections::HashMap;
use std::fmt;

use serde::de::{Deserialize, Deserializer, Error, MapAccess, Visitor};

enum CatalogValue {
    Text(String),
    Group(HashMap<String, CatalogValue>),
}

impl<'de> Deserialize<'de> for CatalogValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct CatalogVisitor;

        impl<'de> Visitor<'de> for CatalogVisitor {
            type Value = CatalogValue;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a translation string or a nonempty namespace object")
            }

            fn visit_str<E: Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(CatalogValue::Text(value.to_owned()))
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut entries = HashMap::new();
                while let Some((key, value)) = map.next_entry::<String, CatalogValue>()? {
                    if key.is_empty() {
                        return Err(M::Error::custom("empty translation key"));
                    }
                    if entries.insert(key.clone(), value).is_some() {
                        return Err(M::Error::custom(format!(
                            "duplicate translation key: {key}"
                        )));
                    }
                }
                if entries.is_empty() {
                    return Err(M::Error::custom("empty translation namespace"));
                }
                Ok(CatalogValue::Group(entries))
            }
        }

        deserializer.deserialize_any(CatalogVisitor)
    }
}

pub(super) fn parse(json: &str) -> Result<HashMap<String, String>, String> {
    fn walk(
        prefix: &str,
        entries: HashMap<String, CatalogValue>,
        output: &mut HashMap<String, String>,
    ) -> Result<(), String> {
        for (name, value) in entries {
            let key = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}.{name}")
            };
            match value {
                CatalogValue::Text(text) => {
                    if output.insert(key.clone(), text).is_some() {
                        return Err(format!("duplicate flattened translation key: {key}"));
                    }
                }
                CatalogValue::Group(entries) => walk(&key, entries, output)?,
            }
        }
        Ok(())
    }

    let value = serde_json::from_str(json).map_err(|error| error.to_string())?;
    let CatalogValue::Group(entries) = value else {
        return Err("catalog root must be a namespace object".to_owned());
    };
    let mut output = HashMap::new();
    walk("", entries, &mut output)?;
    Ok(output)
}

#[test]
fn rejects_duplicate_keys_before_the_json_parser_can_overwrite_them() {
    for json in [
        r#"{"menu":{"open":"Open","open":"Other"}}"#,
        r#"{"menu":{"open":"Open"},"menu":{"close":"Close"}}"#,
        r#"{"menu.open":"Open","menu":{"open":"Other"}}"#,
    ] {
        assert!(parse(json).unwrap_err().contains("duplicate"), "{json}");
    }
}

#[test]
fn rejects_values_that_would_silently_disappear_from_catalog_checks() {
    for json in [
        r#"{"key":null}"#,
        r#"{"key":42}"#,
        r#"{"key":false}"#,
        r#"{"key":[]}"#,
        r#"{"key":{}}"#,
        r#"{"":"Empty key"}"#,
        r#""Not a catalog""#,
    ] {
        assert!(parse(json).is_err(), "{json}");
    }
}
