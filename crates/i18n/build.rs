use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs,
};

fn parameters(text: &str) -> BTreeSet<&str> {
    text.split('{')
        .skip(1)
        .filter_map(|part| part.split_once('}').map(|(key, _)| key))
        .collect()
}

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=locales/en.json");
    println!("cargo:rerun-if-changed=locales/zh-CN.json");
    let english: BTreeMap<String, String> =
        serde_json::from_str(&fs::read_to_string("locales/en.json")?)?;
    let chinese: BTreeMap<String, String> =
        serde_json::from_str(&fs::read_to_string("locales/zh-CN.json")?)?;
    if english.keys().ne(chinese.keys()) {
        return Err("English and Chinese catalogs have different message keys".into());
    }
    for (key, value) in &chinese {
        if value.trim().is_empty() {
            return Err(format!("Empty Chinese translation: {key}").into());
        }
        if parameters(key) != parameters(value) {
            return Err(format!("Translation parameters differ: {key}").into());
        }
    }
    Ok(())
}
