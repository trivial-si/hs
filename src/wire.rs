//! Shared JSON boundary for the CLI and MCP transports.
//!
//! Reject ambiguous objects before a schema or a command can choose a meaning.
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Number, Value};
use std::fmt;

pub const MAX_JSON_BYTES: usize = 1_048_576;

pub fn parse(bytes: &[u8]) -> Result<Value, serde_json::Error> {
    if bytes.len() > MAX_JSON_BYTES {
        return Err(de::Error::custom("JSON message exceeds the 1 MiB limit"));
    }
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let value = UniqueValue::deserialize(&mut decoder)?.0;
    decoder.end()?;
    Ok(value)
}

struct UniqueValue(Value);
impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        decoder.deserialize_any(UniqueVisitor).map(Self)
    }
}
struct UniqueVisitor;
impl<'de> Visitor<'de> for UniqueVisitor {
    type Value = Value;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("JSON without duplicate object keys")
    }
    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Value, E> {
        Ok(Value::Number(v.into()))
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Value, E> {
        Ok(Value::Number(v.into()))
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Value, E> {
        Number::from_f64(v)
            .map(Value::Number)
            .ok_or_else(|| E::custom("JSON numbers must be finite"))
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<Value, E> {
        Ok(Value::String(v.into()))
    }
    fn visit_string<E: de::Error>(self, v: String) -> Result<Value, E> {
        Ok(Value::String(v))
    }
    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_none<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = seq.next_element::<UniqueValue>()? {
            values.push(value.0);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut source: A) -> Result<Value, A::Error> {
        let mut map = Map::new();
        while let Some(key) = source.next_key::<String>()? {
            if map.contains_key(&key) {
                return Err(de::Error::custom(format!(
                    "Duplicate JSON object key {key:?}"
                )));
            }
            map.insert(key, source.next_value::<UniqueValue>()?.0);
        }
        Ok(Value::Object(map))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicate_names_are_rejected_at_every_level_including_escape_equivalence() {
        for bytes in [
            br#"{"operation":"context","operation":"task.add"}"#.as_slice(),
            br#"{"input":{"title":"safe","\u0074itle":"changed"}}"#,
            br#"[{"id":1,"id":2}]"#,
        ] {
            assert!(parse(bytes).is_err());
        }
        assert_eq!(
            parse(br#"{"tags":["same","same"],"nested":{"id":2},"id":1}"#).unwrap()["id"],
            1
        );
    }
    #[test]
    fn transport_limits_and_standard_json_errors_are_preserved() {
        for value in ["{} {}", "[NaN]", "[1,]", "\"unterminated"] {
            assert!(parse(value.as_bytes()).is_err());
        }
        assert!(parse(&vec![b' '; MAX_JSON_BYTES + 1]).is_err());
        assert!(parse(format!("{}0{}", "[".repeat(130), "]".repeat(130)).as_bytes()).is_err());
        assert_eq!(
            parse(b"[null,true,-2,3,1.5]").unwrap(),
            serde_json::json!([null, true, -2, 3, 1.5])
        );
    }
}
