use serde::{
    Deserialize, Deserializer,
    de::{DeserializeOwned, Error, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Number, Value};
use std::{collections::BTreeSet, fmt};

struct StrictValue(Value);

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = StrictValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_bool<E: Error>(self, value: bool) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Bool(value)))
    }

    fn visit_i64<E: Error>(self, value: i64) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Number(value.into())))
    }

    fn visit_u64<E: Error>(self, value: u64) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Number(value.into())))
    }

    fn visit_f64<E: Error>(self, value: f64) -> Result<Self::Value, E> {
        Number::from_f64(value)
            .map(Value::Number)
            .map(StrictValue)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E: Error>(self, value: &str) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::String(value.to_owned())))
    }

    fn visit_string<E: Error>(self, value: String) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::String(value)))
    }

    fn visit_none<E: Error>(self) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Null))
    }

    fn visit_unit<E: Error>(self) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Null))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut values: A) -> Result<Self::Value, A::Error> {
        let mut output = Vec::new();
        while let Some(value) = values.next_element::<StrictValue>()? {
            output.push(value.0);
        }
        Ok(StrictValue(Value::Array(output)))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut values: A) -> Result<Self::Value, A::Error> {
        let mut keys = BTreeSet::new();
        let mut output = Map::new();
        while let Some(key) = values.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                return Err(A::Error::custom(format!("duplicate JSON key: {key}")));
            }
            output.insert(key, values.next_value::<StrictValue>()?.0);
        }
        Ok(StrictValue(Value::Object(output)))
    }
}

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(StrictVisitor)
    }
}

pub fn from_slice<T: DeserializeOwned>(data: &[u8]) -> Result<T, serde_json::Error> {
    let strict = serde_json::from_slice::<StrictValue>(data)?;
    serde_json::from_value(strict.0)
}

pub fn from_str<T: DeserializeOwned>(data: &str) -> Result<T, serde_json::Error> {
    from_slice(data.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_keys_are_rejected_at_every_depth() {
        assert!(from_str::<Value>(r#"{"schema":1,"schema":1}"#).is_err());
        assert!(from_str::<Value>(r#"{"outer":{"value":1,"value":2}}"#).is_err());
        assert!(from_str::<Value>(r#"{"schema":1}"#).is_ok());
    }
}
