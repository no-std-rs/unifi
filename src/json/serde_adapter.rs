//! Reuse the validated JSON string decoder while retaining serde-json-core's data model.
use super::{JsonError, JsonString};
use serde::de::{self, DeserializeSeed, EnumAccess, MapAccess, SeqAccess, VariantAccess, Visitor};

struct WithScratch<'s, T> {
    inner: T,
    scratch: &'s mut [u8],
}

pub(super) fn decode<'de, T: serde::Deserialize<'de>>(
    input: &'de [u8],
    scratch: &mut [u8],
) -> Result<T, JsonError> {
    // Keep escaped strings borrowed until the adapter visits them. The dependency's
    // unescaper rejects surrogate pairs, which Json::parse has already validated.
    let mut decoder = serde_json_core::de::Deserializer::new(input, None);
    let value = T::deserialize(WithScratch {
        inner: &mut decoder,
        scratch,
    })
    .map_err(|_| JsonError::Decode)?;
    decoder.end().map_err(|_| JsonError::Decode)?;
    Ok(value)
}

macro_rules! deserialize {
    ($($method:ident $(($($arg:ident : $ty:ty),*))?),* $(,)?) => {$(
        fn $method<V: Visitor<'de>>(self, $($($arg: $ty,)*)? visitor: V) -> Result<V::Value, Self::Error> {
            self.inner.$method($($($arg,)*)? WithScratch { inner: visitor, scratch: self.scratch })
        }
    )*};
}

impl<'de, D: de::Deserializer<'de>> de::Deserializer<'de> for WithScratch<'_, D> {
    type Error = D::Error;

    deserialize! {
        deserialize_any, deserialize_bool,
        deserialize_i8, deserialize_i16, deserialize_i32, deserialize_i64, deserialize_i128,
        deserialize_u8, deserialize_u16, deserialize_u32, deserialize_u64, deserialize_u128,
        deserialize_f32, deserialize_f64, deserialize_char, deserialize_str, deserialize_string,
        deserialize_bytes, deserialize_byte_buf, deserialize_option, deserialize_unit,
        deserialize_unit_struct(name: &'static str), deserialize_seq,
        deserialize_tuple(len: usize), deserialize_tuple_struct(name: &'static str, len: usize),
        deserialize_map, deserialize_struct(name: &'static str, fields: &'static [&'static str]),
        deserialize_enum(name: &'static str, variants: &'static [&'static str]),
        deserialize_identifier, deserialize_ignored_any,
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        if name == "__serde_json_core_escaped_string__" {
            // Preserve the dependency's explicit opt-in to borrowed escaped text.
            self.inner.deserialize_newtype_struct(name, visitor)
        } else {
            self.inner.deserialize_newtype_struct(
                name,
                WithScratch {
                    inner: visitor,
                    scratch: self.scratch,
                },
            )
        }
    }

    fn is_human_readable(&self) -> bool {
        self.inner.is_human_readable()
    }
}

macro_rules! visit {
    ($($method:ident($ty:ty)),* $(,)?) => {$(
        fn $method<E: de::Error>(self, value: $ty) -> Result<Self::Value, E> {
            self.inner.$method(value)
        }
    )*};
}

impl<'de, V: Visitor<'de>> Visitor<'de> for WithScratch<'_, V> {
    type Value = V::Value;

    fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.inner.expecting(formatter)
    }

    visit! {
        visit_bool(bool), visit_i8(i8), visit_i16(i16), visit_i32(i32), visit_i64(i64), visit_i128(i128),
        visit_u8(u8), visit_u16(u16), visit_u32(u32), visit_u64(u64), visit_u128(u128),
        visit_f32(f32), visit_f64(f64), visit_char(char), visit_str(&str),
        visit_bytes(&[u8]), visit_borrowed_bytes(&'de [u8]),
    }

    fn visit_borrowed_str<E: de::Error>(self, value: &'de str) -> Result<Self::Value, E> {
        if !value.contains('\\') {
            return self.inner.visit_borrowed_str(value);
        }
        let mut used = 0;
        for character in JsonString(value).chars() {
            let mut bytes = [0; 4];
            let bytes = character.encode_utf8(&mut bytes).as_bytes();
            self.scratch
                .get_mut(used..used + bytes.len())
                .ok_or_else(|| E::custom("escaped string exceeds scratch capacity"))?
                .copy_from_slice(bytes);
            used += bytes.len();
        }
        let value = core::str::from_utf8(&self.scratch[..used]).map_err(E::custom)?;
        // Escaped strings are transient; owned bounded strings can consume them,
        // while unescaped strings still borrow directly from the original input.
        self.inner.visit_str(value)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        self.inner.visit_unit()
    }
    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        self.inner.visit_none()
    }
    fn visit_some<D: de::Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        self.inner.visit_some(WithScratch {
            inner: decoder,
            scratch: self.scratch,
        })
    }
    fn visit_newtype_struct<D: de::Deserializer<'de>>(
        self,
        decoder: D,
    ) -> Result<Self::Value, D::Error> {
        self.inner.visit_newtype_struct(WithScratch {
            inner: decoder,
            scratch: self.scratch,
        })
    }
    fn visit_seq<A: SeqAccess<'de>>(self, access: A) -> Result<Self::Value, A::Error> {
        self.inner.visit_seq(WithScratch {
            inner: access,
            scratch: self.scratch,
        })
    }
    fn visit_map<A: MapAccess<'de>>(self, access: A) -> Result<Self::Value, A::Error> {
        self.inner.visit_map(WithScratch {
            inner: access,
            scratch: self.scratch,
        })
    }
    fn visit_enum<A: EnumAccess<'de>>(self, access: A) -> Result<Self::Value, A::Error> {
        self.inner.visit_enum(WithScratch {
            inner: access,
            scratch: self.scratch,
        })
    }
}

impl<'de, S: DeserializeSeed<'de>> DeserializeSeed<'de> for WithScratch<'_, S> {
    type Value = S::Value;
    fn deserialize<D: de::Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        self.inner.deserialize(WithScratch {
            inner: decoder,
            scratch: self.scratch,
        })
    }
}

impl<'de, A: SeqAccess<'de>> SeqAccess<'de> for WithScratch<'_, A> {
    type Error = A::Error;
    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, Self::Error> {
        self.inner.next_element_seed(WithScratch {
            inner: seed,
            scratch: &mut *self.scratch,
        })
    }
    fn size_hint(&self) -> Option<usize> {
        self.inner.size_hint()
    }
}

impl<'de, A: MapAccess<'de>> MapAccess<'de> for WithScratch<'_, A> {
    type Error = A::Error;
    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Self::Error> {
        self.inner.next_key_seed(WithScratch {
            inner: seed,
            scratch: &mut *self.scratch,
        })
    }
    fn next_value_seed<V: DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value, Self::Error> {
        self.inner.next_value_seed(WithScratch {
            inner: seed,
            scratch: &mut *self.scratch,
        })
    }
    fn size_hint(&self) -> Option<usize> {
        self.inner.size_hint()
    }
}

impl<'de, 's, A: EnumAccess<'de>> EnumAccess<'de> for WithScratch<'s, A> {
    type Error = A::Error;
    type Variant = WithScratch<'s, A::Variant>;
    fn variant_seed<V: DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Self::Variant), Self::Error> {
        let (value, variant) = self.inner.variant_seed(WithScratch {
            inner: seed,
            scratch: &mut *self.scratch,
        })?;
        Ok((
            value,
            WithScratch {
                inner: variant,
                scratch: self.scratch,
            },
        ))
    }
}

impl<'de, A: VariantAccess<'de>> VariantAccess<'de> for WithScratch<'_, A> {
    type Error = A::Error;
    fn unit_variant(self) -> Result<(), Self::Error> {
        self.inner.unit_variant()
    }
    fn newtype_variant_seed<T: DeserializeSeed<'de>>(
        self,
        seed: T,
    ) -> Result<T::Value, Self::Error> {
        self.inner.newtype_variant_seed(WithScratch {
            inner: seed,
            scratch: self.scratch,
        })
    }
    fn tuple_variant<V: Visitor<'de>>(
        self,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.inner.tuple_variant(
            len,
            WithScratch {
                inner: visitor,
                scratch: self.scratch,
            },
        )
    }
    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.inner.struct_variant(
            fields,
            WithScratch {
                inner: visitor,
                scratch: self.scratch,
            },
        )
    }
}
