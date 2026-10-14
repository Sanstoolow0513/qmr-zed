use gpui::{App, Global, IntoElement, RenderOnce, SharedString, Window};
use std::{collections::BTreeMap, sync::LazyLock};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Locale {
    #[default]
    English,
    SimplifiedChinese,
}

impl Global for Locale {}

static CHINESE: LazyLock<BTreeMap<String, String>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../locales/zh-CN.json")).unwrap_or_else(|error| {
        log::error!("Unable to read the bundled Chinese catalog: {error}");
        BTreeMap::new()
    })
});

pub fn locale(cx: &App) -> Locale {
    cx.try_global::<Locale>().copied().unwrap_or_default()
}

pub fn lookup(source: &str, locale: Locale) -> Option<&'static str> {
    match locale {
        Locale::English => None,
        Locale::SimplifiedChinese => CHINESE.get(source).map(String::as_str),
    }
}

pub fn static_text(source: &'static str, cx: &App) -> &'static str {
    lookup(source, locale(cx)).unwrap_or(source)
}

pub fn localize_schema(schema: &mut serde_json::Value, locale: Locale) {
    match schema {
        serde_json::Value::Object(properties) => {
            for (key, value) in properties {
                if matches!(
                    key.as_str(),
                    "properties"
                        | "patternProperties"
                        | "$defs"
                        | "definitions"
                        | "dependentSchemas"
                ) {
                    if let Some(schemas) = value.as_object_mut() {
                        for schema in schemas.values_mut() {
                            localize_schema(schema, locale);
                        }
                    }
                    continue;
                }
                if matches!(key.as_str(), "default" | "enum" | "const" | "examples") {
                    continue;
                }
                if matches!(
                    key.as_str(),
                    "title" | "description" | "markdownDescription"
                ) && value.is_string()
                {
                    if let Some(translated) =
                        value.as_str().and_then(|text| lookup(text.trim(), locale))
                    {
                        *value = translated.into();
                    }
                } else {
                    localize_schema(value, locale);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                localize_schema(item, locale);
            }
        }
        _ => {}
    }
}

pub fn text(source: impl Into<SharedString>, cx: &App) -> SharedString {
    let source = source.into();
    lookup(&source, locale(cx)).map_or(source, SharedString::new_static)
}

pub fn format(source: &str, arguments: &[(&str, &str)], cx: &App) -> SharedString {
    format_for_locale(source, arguments, locale(cx)).into()
}

pub fn format_for_locale(source: &str, arguments: &[(&str, &str)], locale: Locale) -> String {
    let template = lookup(source, locale).unwrap_or(source);
    let mut result = String::with_capacity(template.len());
    let mut remaining = template;
    while let Some(start) = remaining.find('{') {
        result.push_str(&remaining[..start]);
        let Some(end) = remaining[start..].find('}').map(|end| start + end) else {
            result.push_str(&remaining[start..]);
            return result;
        };
        let name = &remaining[start + 1..end];
        if let Some((_, value)) = arguments.iter().find(|(key, _)| *key == name) {
            result.push_str(value);
        } else {
            result.push_str(&remaining[start..=end]);
        }
        remaining = &remaining[end + 1..];
    }
    result.push_str(remaining);
    result
}

#[derive(IntoElement)]
pub struct Message(&'static str);

impl Message {
    pub fn new(source: &'static str) -> Self {
        Self(source)
    }
}

impl RenderOnce for Message {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        text(self.0, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn placeholders(value: &str) -> BTreeSet<&str> {
        value
            .split('{')
            .skip(1)
            .filter_map(|part| part.split_once('}').map(|(key, _)| key))
            .collect()
    }

    #[test]
    fn catalogs_have_matching_keys_and_parameters() {
        let english: BTreeMap<String, String> =
            serde_json::from_str(include_str!("../locales/en.json")).unwrap();
        assert_eq!(
            english.keys().collect::<Vec<_>>(),
            CHINESE.keys().collect::<Vec<_>>()
        );
        for (key, translated) in CHINESE.iter() {
            assert!(!translated.trim().is_empty(), "empty translation: {key}");
            assert_eq!(
                placeholders(key),
                placeholders(translated),
                "parameters: {key}"
            );
        }
    }

    #[test]
    fn formatting_preserves_user_arguments() {
        assert_eq!(
            format_for_locale(
                "Open {path}",
                &[("path", "Save {name}.rs")],
                Locale::SimplifiedChinese
            ),
            "打开 Save {name}.rs"
        );
        assert_eq!(
            format_for_locale("Unknown {path}", &[("path", "中文.rs")], Locale::English),
            "Unknown 中文.rs"
        );
    }

    #[test]
    fn schema_localization_preserves_configuration_values() {
        let mut schema = serde_json::json!({
            "description": "Save",
            "enum": ["Save"],
            "default": {"title": "Save"},
            "properties": {"title": {"description": "Save", "const": "Save"}}
        });
        localize_schema(&mut schema, Locale::SimplifiedChinese);
        assert_eq!(schema["description"], "保存");
        assert_eq!(schema["enum"][0], "Save");
        assert_eq!(schema["default"]["title"], "Save");
        assert_eq!(schema["properties"]["title"]["description"], "保存");
        assert_eq!(schema["properties"]["title"]["const"], "Save");
    }

    #[gpui::test]
    fn locales_are_isolated_between_applications(
        first: &mut gpui::TestAppContext,
        second: &mut gpui::TestAppContext,
    ) {
        first.update(|cx| cx.set_global(Locale::SimplifiedChinese));
        first.read(|cx| assert_eq!(text("Save", cx).as_ref(), "保存"));
        second.read(|cx| assert_eq!(text("Save", cx).as_ref(), "Save"));
    }
}
