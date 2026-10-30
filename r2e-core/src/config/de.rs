//! A serde `Deserializer` over [`ConfigValue`] that coerces strings the way
//! [`FromConfigValue`](super::FromConfigValue) does.
//!
//! Every `R2E_*` override and every resolved `${VAR}` placeholder lands in the
//! config map as a `ConfigValue::String`. Deserializing through
//! `serde_json::Value` would reject `"8080"` for a `u16` field, making env
//! overrides second-class for serde-typed config. Here the **typed hints**
//! coerce:
//!
//! - `deserialize_{i*,u*}` / `deserialize_f*` / `deserialize_bool` on a
//!   `String` parse it (bool: `true`/`1`/`yes`, `false`/`0`/`no`,
//!   case-insensitive);
//! - `deserialize_str` / `deserialize_string` accept an integer, float or bool
//!   and format it;
//! - `deserialize_seq` on a single scalar yields a one-element sequence
//!   (`Vec<T>` from one value, like `FromConfigValue for Vec<T>`);
//! - `deserialize_option` on `Null` yields `None`; struct / map / seq hints on
//!   `Null` read as empty (an empty YAML section `tls:` is an absent one).
//!
//! `deserialize_any` never coerces, so `#[serde(untagged)]` enums, internally
//! tagged enums, `#[serde(flatten)]` and `serde_json::Value` fields see the
//! value as stored. Map keys are strings and are not parsed.

use std::collections::hash_map;
use std::fmt;

use serde::de::value::BorrowedStrDeserializer;
use serde::de::{
    self, DeserializeSeed, EnumAccess, IntoDeserializer, MapAccess, SeqAccess, VariantAccess,
    Visitor,
};

use super::value::ConfigValue;

/// Error produced by [`ValueDeserializer`].
#[derive(Debug)]
pub(crate) struct DeError(String);

impl fmt::Display for DeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DeError {}

impl de::Error for DeError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        DeError(msg.to_string())
    }
}

/// Deserialize `T` from a borrowed [`ConfigValue`].
pub(crate) fn from_value<'de, T: de::Deserialize<'de>>(
    value: &'de ConfigValue,
) -> Result<T, DeError> {
    T::deserialize(ValueDeserializer(value))
}

/// Serde deserializer over a borrowed [`ConfigValue`].
#[derive(Clone, Copy)]
pub(crate) struct ValueDeserializer<'de>(pub(crate) &'de ConfigValue);

fn parse_bool(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

macro_rules! coerce_signed {
    ($($method:ident),+) => {
        $(
            fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
                match self.0 {
                    ConfigValue::String(s) => match s.parse::<i64>() {
                        Ok(i) => visitor.visit_i64(i),
                        Err(_) => self.deserialize_any(visitor),
                    },
                    _ => self.deserialize_any(visitor),
                }
            }
        )+
    };
}

macro_rules! coerce_unsigned {
    ($($method:ident),+) => {
        $(
            fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
                match self.0 {
                    ConfigValue::String(s) => {
                        if let Ok(i) = s.parse::<i64>() {
                            visitor.visit_i64(i)
                        } else if let Ok(u) = s.parse::<u64>() {
                            visitor.visit_u64(u)
                        } else {
                            self.deserialize_any(visitor)
                        }
                    }
                    _ => self.deserialize_any(visitor),
                }
            }
        )+
    };
}

macro_rules! coerce_float {
    ($($method:ident),+) => {
        $(
            fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
                match self.0 {
                    ConfigValue::String(s) => match s.parse::<f64>() {
                        Ok(f) => visitor.visit_f64(f),
                        Err(_) => self.deserialize_any(visitor),
                    },
                    _ => self.deserialize_any(visitor),
                }
            }
        )+
    };
}

impl<'de> de::Deserializer<'de> for ValueDeserializer<'de> {
    type Error = DeError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        match self.0 {
            ConfigValue::String(s) => visitor.visit_borrowed_str(s),
            ConfigValue::Integer(i) => visitor.visit_i64(*i),
            ConfigValue::Float(f) => visitor.visit_f64(*f),
            ConfigValue::Bool(b) => visitor.visit_bool(*b),
            ConfigValue::Null => visitor.visit_unit(),
            ConfigValue::List(items) => visit_seq(items, visitor),
            ConfigValue::Map(map) => visit_map(map.iter(), visitor),
        }
    }

    coerce_signed!(
        deserialize_i8,
        deserialize_i16,
        deserialize_i32,
        deserialize_i64
    );
    coerce_unsigned!(
        deserialize_u8,
        deserialize_u16,
        deserialize_u32,
        deserialize_u64
    );
    coerce_float!(deserialize_f32, deserialize_f64);

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        match self.0 {
            ConfigValue::String(s) => match parse_bool(s) {
                Some(b) => visitor.visit_bool(b),
                None => self.deserialize_any(visitor),
            },
            _ => self.deserialize_any(visitor),
        }
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        match self.0 {
            ConfigValue::Integer(i) => visitor.visit_string(i.to_string()),
            ConfigValue::Float(f) => visitor.visit_string(f.to_string()),
            ConfigValue::Bool(b) => visitor.visit_string(b.to_string()),
            _ => self.deserialize_any(visitor),
        }
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        self.deserialize_str(visitor)
    }

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        self.deserialize_str(visitor)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        match self.0 {
            ConfigValue::Null => visitor.visit_none(),
            _ => visitor.visit_some(self),
        }
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        self.deserialize_any(visitor)
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, DeError> {
        self.deserialize_unit(visitor)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, DeError> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        match self.0 {
            ConfigValue::List(items) => visit_seq(items, visitor),
            ConfigValue::Null => visit_seq(&[], visitor),
            ConfigValue::Map(_) => self.deserialize_any(visitor),
            scalar => visit_seq(std::slice::from_ref(scalar), visitor),
        }
    }

    fn deserialize_tuple<V: Visitor<'de>>(
        self,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, DeError> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, DeError> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        match self.0 {
            ConfigValue::Null => visit_map(Default::default(), visitor),
            _ => self.deserialize_any(visitor),
        }
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, DeError> {
        self.deserialize_map(visitor)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, DeError> {
        match self.0 {
            ConfigValue::String(s) => visitor.visit_enum(BorrowedStrDeserializer::new(s)),
            ConfigValue::Map(map) if map.len() == 1 => {
                let (variant, value) = map.iter().next().expect("len == 1");
                visitor.visit_enum(EnumDeserializer { variant, value })
            }
            other => Err(de::Error::invalid_type(unexpected(other), &visitor)),
        }
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        self.deserialize_str(visitor)
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        self.deserialize_any(visitor)
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        self.deserialize_any(visitor)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        visitor.visit_unit()
    }
}

fn unexpected(value: &ConfigValue) -> de::Unexpected<'_> {
    match value {
        ConfigValue::String(s) => de::Unexpected::Str(s),
        ConfigValue::Integer(i) => de::Unexpected::Signed(*i),
        ConfigValue::Float(f) => de::Unexpected::Float(*f),
        ConfigValue::Bool(b) => de::Unexpected::Bool(*b),
        ConfigValue::Null => de::Unexpected::Unit,
        ConfigValue::List(_) => de::Unexpected::Seq,
        ConfigValue::Map(_) => de::Unexpected::Map,
    }
}

fn visit_seq<'de, V: Visitor<'de>>(
    items: &'de [ConfigValue],
    visitor: V,
) -> Result<V::Value, DeError> {
    let mut seq = SeqDeserializer {
        iter: items.iter(),
        consumed: 0,
    };
    let value = visitor.visit_seq(&mut seq)?;
    let remaining = seq.iter.len();
    if remaining == 0 {
        Ok(value)
    } else {
        Err(de::Error::invalid_length(
            seq.consumed + remaining,
            &"fewer elements in sequence",
        ))
    }
}

fn visit_map<'de, V: Visitor<'de>>(
    iter: hash_map::Iter<'de, String, ConfigValue>,
    visitor: V,
) -> Result<V::Value, DeError> {
    visitor.visit_map(MapDeserializer { iter, value: None })
}

struct SeqDeserializer<'de> {
    iter: std::slice::Iter<'de, ConfigValue>,
    consumed: usize,
}

impl<'de> SeqAccess<'de> for SeqDeserializer<'de> {
    type Error = DeError;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, DeError> {
        match self.iter.next() {
            Some(value) => {
                self.consumed += 1;
                seed.deserialize(ValueDeserializer(value)).map(Some)
            }
            None => Ok(None),
        }
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.iter.len())
    }
}

struct MapDeserializer<'de> {
    iter: hash_map::Iter<'de, String, ConfigValue>,
    value: Option<&'de ConfigValue>,
}

impl<'de> MapAccess<'de> for MapDeserializer<'de> {
    type Error = DeError;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, DeError> {
        match self.iter.next() {
            Some((key, value)) => {
                self.value = Some(value);
                seed.deserialize(BorrowedStrDeserializer::new(key))
                    .map(Some)
            }
            None => Ok(None),
        }
    }

    fn next_value_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<T::Value, DeError> {
        let value = self
            .value
            .take()
            .ok_or_else(|| <DeError as de::Error>::custom("value requested before key"))?;
        seed.deserialize(ValueDeserializer(value))
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.iter.len())
    }
}

/// Externally tagged enum: `{ variant: value }`.
struct EnumDeserializer<'de> {
    variant: &'de str,
    value: &'de ConfigValue,
}

impl<'de> EnumAccess<'de> for EnumDeserializer<'de> {
    type Error = DeError;
    type Variant = VariantDeserializer<'de>;

    fn variant_seed<V: DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Self::Variant), DeError> {
        let variant = seed.deserialize(self.variant.into_deserializer())?;
        Ok((variant, VariantDeserializer(self.value)))
    }
}

struct VariantDeserializer<'de>(&'de ConfigValue);

impl<'de> VariantAccess<'de> for VariantDeserializer<'de> {
    type Error = DeError;

    fn unit_variant(self) -> Result<(), DeError> {
        match self.0 {
            ConfigValue::Null => Ok(()),
            other => Err(de::Error::invalid_type(unexpected(other), &"unit variant")),
        }
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, DeError> {
        seed.deserialize(ValueDeserializer(self.0))
    }

    fn tuple_variant<V: Visitor<'de>>(self, _len: usize, visitor: V) -> Result<V::Value, DeError> {
        de::Deserializer::deserialize_seq(ValueDeserializer(self.0), visitor)
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, DeError> {
        de::Deserializer::deserialize_map(ValueDeserializer(self.0), visitor)
    }
}
