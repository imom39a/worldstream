use std::{collections::BTreeMap, fmt, str};

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, DeserializeOwned, MapAccess, SeqAccess, Visitor},
    ser::{
        self, SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
        SerializeTupleStruct, SerializeTupleVariant,
    },
};
use thiserror::Error;

/// The largest integer that is interoperable with JSON implementations using
/// IEEE-754 binary64 numbers.
pub const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
/// The smallest interoperable JSON integer.
pub const MIN_SAFE_INTEGER: i64 = -MAX_SAFE_INTEGER;

/// A JSON value admitted by `worldstream/canonical-json/v1`.
///
/// Construction always validates the full value tree. In particular, there is
/// no path for a float, duplicate object key, or unsafe integer to enter this
/// type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalJsonV1(Node);

impl CanonicalJsonV1 {
    /// Parses JSON and returns its canonical value. The input itself need not
    /// already have canonical whitespace or object-key order.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid UTF-8/JSON, duplicate keys, floats, or
    /// integers outside the interoperable safe range.
    pub fn parse(input: &[u8]) -> Result<Self, CanonicalJsonError> {
        str::from_utf8(input).map_err(|_| CanonicalJsonError::InvalidUtf8)?;
        let mut deserializer = serde_json::Deserializer::from_slice(input);
        let node = Node::deserialize(&mut deserializer)
            .map_err(|error| CanonicalJsonError::InvalidJson(error.to_string()))?;
        deserializer
            .end()
            .map_err(|error| CanonicalJsonError::InvalidJson(error.to_string()))?;
        Ok(Self(node))
    }

    /// Reads a stored canonical value and rejects any byte representation that
    /// does not round-trip byte-for-byte through the v1 writer.
    ///
    /// # Errors
    ///
    /// Returns an error when parsing fails or the original bytes are not the
    /// unique canonical encoding.
    pub fn from_canonical_bytes(input: &[u8]) -> Result<Self, CanonicalJsonError> {
        let value = Self::parse(input)?;
        if value.to_bytes()? != input {
            return Err(CanonicalJsonError::NonCanonicalBytes);
        }
        Ok(value)
    }

    /// Converts one of this crate's closed, derived typed models through the
    /// checked canonical writer. This is intentionally not public: a general
    /// custom `SerializeMap` can emit duplicate keys before `serde_json::Value`
    /// observes them.
    pub(crate) fn from_serialize<T: Serialize>(value: &T) -> Result<Self, CanonicalJsonError> {
        value
            .serialize(NodeSerializer)
            .map(Self)
            .map_err(|error| CanonicalJsonError::Serialization(error.to_string()))
    }

    /// Returns the unique canonical JSON byte representation.
    ///
    /// # Errors
    ///
    /// Returns an error if the internally validated value cannot be written.
    pub fn to_bytes(&self) -> Result<Vec<u8>, CanonicalJsonError> {
        let mut output = Vec::new();
        self.0.write(&mut output)?;
        Ok(output)
    }

    /// Strictly decodes an already-canonical typed command.
    ///
    /// Callers should put `#[serde(deny_unknown_fields)]` on command models.
    /// Every `WorldStream` Core command model does so.
    ///
    /// # Errors
    ///
    /// Returns an error for noncanonical bytes or a typed decoding failure.
    pub fn decode_canonical<T: DeserializeOwned>(input: &[u8]) -> Result<T, CanonicalJsonError> {
        let value = Self::from_canonical_bytes(input)?;
        let bytes = value.to_bytes()?;
        serde_json::from_slice(&bytes)
            .map_err(|error| CanonicalJsonError::TypedDecode(error.to_string()))
    }
}

impl Serialize for CanonicalJsonV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for CanonicalJsonV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Node::deserialize(deserializer).map(Self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Node {
    Null,
    Bool(bool),
    Integer(i64),
    String(String),
    Array(Vec<Self>),
    Object(BTreeMap<String, Self>),
}

impl Node {
    fn checked_integer<E: de::Error>(value: i128) -> Result<Self, E> {
        if (i128::from(MIN_SAFE_INTEGER)..=i128::from(MAX_SAFE_INTEGER)).contains(&value) {
            #[allow(clippy::cast_possible_truncation)]
            Ok(Self::Integer(value as i64))
        } else {
            Err(E::custom(format_args!(
                "integer {value} is outside the canonical safe range"
            )))
        }
    }

    fn write(&self, output: &mut Vec<u8>) -> Result<(), CanonicalJsonError> {
        match self {
            Self::Null => output.extend_from_slice(b"null"),
            Self::Bool(false) => output.extend_from_slice(b"false"),
            Self::Bool(true) => output.extend_from_slice(b"true"),
            Self::Integer(value) => output.extend_from_slice(value.to_string().as_bytes()),
            Self::String(value) => {
                serde_json::to_writer(output, value)
                    .map_err(|error| CanonicalJsonError::Serialization(error.to_string()))?;
            }
            Self::Array(values) => {
                output.push(b'[');
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        output.push(b',');
                    }
                    value.write(output)?;
                }
                output.push(b']');
            }
            Self::Object(values) => {
                output.push(b'{');
                for (index, (key, value)) in values.iter().enumerate() {
                    if index != 0 {
                        output.push(b',');
                    }
                    serde_json::to_writer(&mut *output, key)
                        .map_err(|error| CanonicalJsonError::Serialization(error.to_string()))?;
                    output.push(b':');
                    value.write(output)?;
                }
                output.push(b'}');
            }
        }
        Ok(())
    }
}

impl Serialize for Node {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Null => serializer.serialize_unit(),
            Self::Bool(value) => serializer.serialize_bool(*value),
            Self::Integer(value) => serializer.serialize_i64(*value),
            Self::String(value) => serializer.serialize_str(value),
            Self::Array(values) => values.serialize(serializer),
            Self::Object(values) => values.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Node {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(NodeVisitor)
    }
}

struct NodeVisitor;

impl<'de> Visitor<'de> for NodeVisitor {
    type Value = Node;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("canonical JSON without floats, duplicate keys, or unsafe integers")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(Node::Null)
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(Node::Null)
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
        Ok(Node::Bool(value))
    }

    fn visit_i8<E: de::Error>(self, value: i8) -> Result<Self::Value, E> {
        Node::checked_integer(i128::from(value))
    }

    fn visit_i16<E: de::Error>(self, value: i16) -> Result<Self::Value, E> {
        Node::checked_integer(i128::from(value))
    }

    fn visit_i32<E: de::Error>(self, value: i32) -> Result<Self::Value, E> {
        Node::checked_integer(i128::from(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
        Node::checked_integer(i128::from(value))
    }

    fn visit_i128<E: de::Error>(self, value: i128) -> Result<Self::Value, E> {
        Node::checked_integer(value)
    }

    fn visit_u8<E: de::Error>(self, value: u8) -> Result<Self::Value, E> {
        Node::checked_integer(i128::from(value))
    }

    fn visit_u16<E: de::Error>(self, value: u16) -> Result<Self::Value, E> {
        Node::checked_integer(i128::from(value))
    }

    fn visit_u32<E: de::Error>(self, value: u32) -> Result<Self::Value, E> {
        Node::checked_integer(i128::from(value))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
        Node::checked_integer(i128::from(value))
    }

    fn visit_u128<E: de::Error>(self, value: u128) -> Result<Self::Value, E> {
        let signed = i128::try_from(value)
            .map_err(|_| E::custom("integer is outside the canonical safe range"))?;
        Node::checked_integer(signed)
    }

    fn visit_f32<E: de::Error>(self, _value: f32) -> Result<Self::Value, E> {
        Err(E::custom("floating point numbers are forbidden"))
    }

    fn visit_f64<E: de::Error>(self, _value: f64) -> Result<Self::Value, E> {
        Err(E::custom("floating point numbers are forbidden"))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        Ok(Node::String(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
        Ok(Node::String(value))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        let mut values = Vec::with_capacity(sequence.size_hint().unwrap_or(0));
        while let Some(value) = sequence.next_element()? {
            values.push(value);
        }
        Ok(Node::Array(values))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut values = BTreeMap::new();
        while let Some((key, value)) = map.next_entry::<String, Node>()? {
            if values.insert(key.clone(), value).is_some() {
                return Err(de::Error::custom(format_args!(
                    "duplicate object key {key:?}"
                )));
            }
        }
        Ok(Node::Object(values))
    }
}

struct NodeSerializer;

impl Serializer for NodeSerializer {
    type Ok = Node;
    type Error = NodeSerializeError;
    type SerializeSeq = SequenceSerializer;
    type SerializeTuple = SequenceSerializer;
    type SerializeTupleStruct = SequenceSerializer;
    type SerializeTupleVariant = TupleVariantSerializer;
    type SerializeMap = MapSerializer;
    type SerializeStruct = MapSerializer;
    type SerializeStructVariant = StructVariantSerializer;

    fn serialize_bool(self, value: bool) -> Result<Self::Ok, Self::Error> {
        Ok(Node::Bool(value))
    }

    fn serialize_i8(self, value: i8) -> Result<Self::Ok, Self::Error> {
        serialize_signed(i128::from(value))
    }

    fn serialize_i16(self, value: i16) -> Result<Self::Ok, Self::Error> {
        serialize_signed(i128::from(value))
    }

    fn serialize_i32(self, value: i32) -> Result<Self::Ok, Self::Error> {
        serialize_signed(i128::from(value))
    }

    fn serialize_i64(self, value: i64) -> Result<Self::Ok, Self::Error> {
        serialize_signed(i128::from(value))
    }

    fn serialize_i128(self, value: i128) -> Result<Self::Ok, Self::Error> {
        serialize_signed(value)
    }

    fn serialize_u8(self, value: u8) -> Result<Self::Ok, Self::Error> {
        serialize_unsigned(u128::from(value))
    }

    fn serialize_u16(self, value: u16) -> Result<Self::Ok, Self::Error> {
        serialize_unsigned(u128::from(value))
    }

    fn serialize_u32(self, value: u32) -> Result<Self::Ok, Self::Error> {
        serialize_unsigned(u128::from(value))
    }

    fn serialize_u64(self, value: u64) -> Result<Self::Ok, Self::Error> {
        serialize_unsigned(u128::from(value))
    }

    fn serialize_u128(self, value: u128) -> Result<Self::Ok, Self::Error> {
        serialize_unsigned(value)
    }

    fn serialize_f32(self, _value: f32) -> Result<Self::Ok, Self::Error> {
        Err(NodeSerializeError::new(
            "floating point numbers are forbidden",
        ))
    }

    fn serialize_f64(self, _value: f64) -> Result<Self::Ok, Self::Error> {
        Err(NodeSerializeError::new(
            "floating point numbers are forbidden",
        ))
    }

    fn serialize_char(self, value: char) -> Result<Self::Ok, Self::Error> {
        Ok(Node::String(value.to_string()))
    }

    fn serialize_str(self, value: &str) -> Result<Self::Ok, Self::Error> {
        Ok(Node::String(value.to_owned()))
    }

    fn serialize_bytes(self, _value: &[u8]) -> Result<Self::Ok, Self::Error> {
        Err(NodeSerializeError::new(
            "raw bytes require an explicit versioned text representation",
        ))
    }

    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        Ok(Node::Null)
    }

    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<Self::Ok, Self::Error> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        Ok(Node::Null)
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        Ok(Node::Null)
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        Ok(Node::String(variant.to_owned()))
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        let mut values = BTreeMap::new();
        values.insert(variant.to_owned(), value.serialize(NodeSerializer)?);
        Ok(Node::Object(values))
    }

    fn serialize_seq(self, length: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        Ok(SequenceSerializer::new(length))
    }

    fn serialize_tuple(self, length: usize) -> Result<Self::SerializeTuple, Self::Error> {
        Ok(SequenceSerializer::new(Some(length)))
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        length: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        Ok(SequenceSerializer::new(Some(length)))
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        length: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        Ok(TupleVariantSerializer {
            variant,
            values: Vec::with_capacity(length),
        })
    }

    fn serialize_map(self, length: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        Ok(MapSerializer::new(length))
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        length: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        Ok(MapSerializer::new(Some(length)))
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        length: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        Ok(StructVariantSerializer {
            variant,
            values: BTreeMap::new(),
            expected_length: length,
        })
    }
}

fn serialize_signed(value: i128) -> Result<Node, NodeSerializeError> {
    if (i128::from(MIN_SAFE_INTEGER)..=i128::from(MAX_SAFE_INTEGER)).contains(&value) {
        let integer = i64::try_from(value)
            .map_err(|_| NodeSerializeError::new("integer is outside the canonical safe range"))?;
        Ok(Node::Integer(integer))
    } else {
        Err(NodeSerializeError::new(
            "integer is outside the canonical safe range",
        ))
    }
}

fn serialize_unsigned(value: u128) -> Result<Node, NodeSerializeError> {
    let signed = i128::try_from(value)
        .map_err(|_| NodeSerializeError::new("integer is outside the canonical safe range"))?;
    serialize_signed(signed)
}

struct SequenceSerializer {
    values: Vec<Node>,
}

impl SequenceSerializer {
    fn new(length: Option<usize>) -> Self {
        Self {
            values: Vec::with_capacity(length.unwrap_or(0)),
        }
    }

    fn push<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), NodeSerializeError> {
        self.values.push(value.serialize(NodeSerializer)?);
        Ok(())
    }

    fn finish(self) -> Node {
        Node::Array(self.values)
    }
}

impl SerializeSeq for SequenceSerializer {
    type Ok = Node;
    type Error = NodeSerializeError;

    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.push(value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(self.finish())
    }
}

impl SerializeTuple for SequenceSerializer {
    type Ok = Node;
    type Error = NodeSerializeError;

    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.push(value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(self.finish())
    }
}

impl SerializeTupleStruct for SequenceSerializer {
    type Ok = Node;
    type Error = NodeSerializeError;

    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.push(value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(self.finish())
    }
}

struct TupleVariantSerializer {
    variant: &'static str,
    values: Vec<Node>,
}

impl SerializeTupleVariant for TupleVariantSerializer {
    type Ok = Node;
    type Error = NodeSerializeError;

    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.values.push(value.serialize(NodeSerializer)?);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        let mut object = BTreeMap::new();
        object.insert(self.variant.to_owned(), Node::Array(self.values));
        Ok(Node::Object(object))
    }
}

struct MapSerializer {
    values: BTreeMap<String, Node>,
    next_key: Option<String>,
}

impl MapSerializer {
    fn new(length: Option<usize>) -> Self {
        let _ = length;
        Self {
            values: BTreeMap::new(),
            next_key: None,
        }
    }

    fn insert<T: ?Sized + Serialize>(
        &mut self,
        key: String,
        value: &T,
    ) -> Result<(), NodeSerializeError> {
        if self.values.contains_key(&key) {
            return Err(NodeSerializeError::new(format!(
                "duplicate object key {key:?}"
            )));
        }
        self.values.insert(key, value.serialize(NodeSerializer)?);
        Ok(())
    }
}

impl SerializeMap for MapSerializer {
    type Ok = Node;
    type Error = NodeSerializeError;

    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), Self::Error> {
        if self.next_key.is_some() {
            return Err(NodeSerializeError::new("map key has no value"));
        }
        self.next_key = Some(key.serialize(KeySerializer)?);
        Ok(())
    }

    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        let key = self
            .next_key
            .take()
            .ok_or_else(|| NodeSerializeError::new("map value has no key"))?;
        self.insert(key, value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        if self.next_key.is_some() {
            return Err(NodeSerializeError::new("map key has no value"));
        }
        Ok(Node::Object(self.values))
    }
}

impl SerializeStruct for MapSerializer {
    type Ok = Node;
    type Error = NodeSerializeError;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        self.insert(key.to_owned(), value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(Node::Object(self.values))
    }
}

struct StructVariantSerializer {
    variant: &'static str,
    values: BTreeMap<String, Node>,
    expected_length: usize,
}

impl SerializeStructVariant for StructVariantSerializer {
    type Ok = Node;
    type Error = NodeSerializeError;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        if self.values.contains_key(key) {
            return Err(NodeSerializeError::new(format!(
                "duplicate object key {key:?}"
            )));
        }
        self.values
            .insert(key.to_owned(), value.serialize(NodeSerializer)?);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        let _ = self.expected_length;
        let mut outer = BTreeMap::new();
        outer.insert(self.variant.to_owned(), Node::Object(self.values));
        Ok(Node::Object(outer))
    }
}

struct KeySerializer;

impl Serializer for KeySerializer {
    type Ok = String;
    type Error = NodeSerializeError;
    type SerializeSeq = ser::Impossible<String, NodeSerializeError>;
    type SerializeTuple = ser::Impossible<String, NodeSerializeError>;
    type SerializeTupleStruct = ser::Impossible<String, NodeSerializeError>;
    type SerializeTupleVariant = ser::Impossible<String, NodeSerializeError>;
    type SerializeMap = ser::Impossible<String, NodeSerializeError>;
    type SerializeStruct = ser::Impossible<String, NodeSerializeError>;
    type SerializeStructVariant = ser::Impossible<String, NodeSerializeError>;

    fn serialize_str(self, value: &str) -> Result<Self::Ok, Self::Error> {
        Ok(value.to_owned())
    }

    fn serialize_char(self, value: char) -> Result<Self::Ok, Self::Error> {
        Ok(value.to_string())
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        Ok(variant.to_owned())
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        value.serialize(self)
    }

    fn serialize_bool(self, _value: bool) -> Result<Self::Ok, Self::Error> {
        Err(key_must_be_string())
    }

    fn serialize_i8(self, _value: i8) -> Result<Self::Ok, Self::Error> {
        Err(key_must_be_string())
    }

    fn serialize_i16(self, _value: i16) -> Result<Self::Ok, Self::Error> {
        Err(key_must_be_string())
    }

    fn serialize_i32(self, _value: i32) -> Result<Self::Ok, Self::Error> {
        Err(key_must_be_string())
    }

    fn serialize_i64(self, _value: i64) -> Result<Self::Ok, Self::Error> {
        Err(key_must_be_string())
    }

    fn serialize_i128(self, _value: i128) -> Result<Self::Ok, Self::Error> {
        Err(key_must_be_string())
    }

    fn serialize_u8(self, _value: u8) -> Result<Self::Ok, Self::Error> {
        Err(key_must_be_string())
    }

    fn serialize_u16(self, _value: u16) -> Result<Self::Ok, Self::Error> {
        Err(key_must_be_string())
    }

    fn serialize_u32(self, _value: u32) -> Result<Self::Ok, Self::Error> {
        Err(key_must_be_string())
    }

    fn serialize_u64(self, _value: u64) -> Result<Self::Ok, Self::Error> {
        Err(key_must_be_string())
    }

    fn serialize_u128(self, _value: u128) -> Result<Self::Ok, Self::Error> {
        Err(key_must_be_string())
    }

    fn serialize_f32(self, _value: f32) -> Result<Self::Ok, Self::Error> {
        Err(key_must_be_string())
    }

    fn serialize_f64(self, _value: f64) -> Result<Self::Ok, Self::Error> {
        Err(key_must_be_string())
    }

    fn serialize_bytes(self, _value: &[u8]) -> Result<Self::Ok, Self::Error> {
        Err(key_must_be_string())
    }

    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        Err(key_must_be_string())
    }

    fn serialize_some<T: ?Sized + Serialize>(self, _value: &T) -> Result<Self::Ok, Self::Error> {
        Err(key_must_be_string())
    }

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        Err(key_must_be_string())
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        Err(key_must_be_string())
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        Err(key_must_be_string())
    }

    fn serialize_seq(self, _length: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        Err(key_must_be_string())
    }

    fn serialize_tuple(self, _length: usize) -> Result<Self::SerializeTuple, Self::Error> {
        Err(key_must_be_string())
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        Err(key_must_be_string())
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        Err(key_must_be_string())
    }

    fn serialize_map(self, _length: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        Err(key_must_be_string())
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        Err(key_must_be_string())
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        Err(key_must_be_string())
    }
}

fn key_must_be_string() -> NodeSerializeError {
    NodeSerializeError::new("canonical JSON object keys must serialize as strings")
}

#[derive(Debug)]
struct NodeSerializeError(String);

impl NodeSerializeError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for NodeSerializeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for NodeSerializeError {}

impl ser::Error for NodeSerializeError {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self(message.to_string())
    }
}

/// A canonical JSON codec failure.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum CanonicalJsonError {
    /// JSON text is UTF-8 only.
    #[error("canonical JSON must be UTF-8")]
    InvalidUtf8,
    /// Syntax, duplicate keys, floats, or integer bounds failed during parsing.
    #[error("invalid canonical JSON: {0}")]
    InvalidJson(String),
    /// A stored/history value was valid JSON but was not in its unique encoding.
    #[error("stored JSON bytes are not the canonical v1 encoding")]
    NonCanonicalBytes,
    /// A typed serializer attempted to emit floating point JSON.
    #[error("floating point numbers are forbidden")]
    FloatForbidden,
    /// A typed serializer attempted to emit an integer outside the safe range.
    #[error("integer is outside the canonical safe range")]
    UnsafeInteger,
    /// A typed value could not be represented as JSON.
    #[error("cannot serialize canonical JSON: {0}")]
    Serialization(String),
    /// Canonical bytes did not satisfy the requested strict command schema.
    #[error("canonical command does not match its schema: {0}")]
    TypedDecode(String),
}

pub(crate) fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, CanonicalJsonError> {
    CanonicalJsonV1::from_serialize(value)?.to_bytes()
}
