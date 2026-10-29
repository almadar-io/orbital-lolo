//! A program as printed text: canonical `.orb` JSON and the generated-file view `orb emit` shows.

use orbital_core_base::schema::OrbitalSchema;

pub mod generated;

/// The canonical `.orb` JSON text of `schema`: the bytes `orb emit orb --output <file>` writes.
pub fn canonical_orb_json(schema: &OrbitalSchema) -> Result<String, serde_json::Error> {
    let value = serde_json::to_value(schema)?;
    serde_json::to_string_pretty(&orbital_core_eval::util::canonical_json(&value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_json_sorts_keys_and_is_stable() {
        let schema =
            orbital_core_base::schema::parse_schema(r#"{"orbitals":[],"name":"z","version":"1"}"#)
                .unwrap();
        let text = canonical_orb_json(&schema).unwrap();
        assert_eq!(text, canonical_orb_json(&schema).unwrap());
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted, "keys are sorted");
        assert!(text.starts_with("{\n  \""), "pretty printed");
    }
}
