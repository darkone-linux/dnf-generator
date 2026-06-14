//! YAML helper(s) used before the config is deserialized into the typed
//! [`schema`](crate::nix_generator::schema).

use serde_yaml::Value;

/// Recursively merge two YAML values; `overlay` keys win over `base`.
///
/// Used to layer `var/generated/config.yaml` on top of `etc/config.yaml` before
/// the merged tree is validated against the strict schema.
pub fn deep_merge(base: Value, overlay: Value) -> Value {
    match (base, overlay) {
        (Value::Mapping(mut b), Value::Mapping(o)) => {
            for (k, v) in o {
                let merged = match b.remove(&k) {
                    Some(bv) => deep_merge(bv, v),
                    None => v,
                };
                b.insert(k, merged);
            }
            Value::Mapping(b)
        }
        (_, o) => o,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yaml(src: &str) -> Value {
        serde_yaml::from_str(src).unwrap()
    }

    #[test]
    fn deep_merge_overlay_wins() {
        let base = yaml("a: 1\nb:\n  x: old\n  y: kept\n");
        let overlay = yaml("b:\n  x: new\nc: 3\n");
        let merged = deep_merge(base, overlay);
        let m = merged.as_mapping().unwrap();
        let b = m.get(Value::from("b")).unwrap().as_mapping().unwrap();
        assert_eq!(b.get(Value::from("x")).unwrap().as_str(), Some("new"));
        assert_eq!(b.get(Value::from("y")).unwrap().as_str(), Some("kept"));
        assert_eq!(m.get(Value::from("c")).unwrap().as_i64(), Some(3));
    }
}
