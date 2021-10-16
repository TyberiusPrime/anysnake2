//from serde-with 1.10.0
//use super::*;
use std::marker::PhantomData;
//use serde::ser::{Serialize, Serializer};
use serde::de::{
    Deserialize, Deserializer, Error, MapAccess, Visitor,
};
use std::fmt;
use std::hash::{BuildHasher, Hash};
use std::collections::HashMap;
//use crate::duplicate_key_impls::PreventDuplicateInsertsMap;
//
//
pub trait PreventDuplicateInsertsMap<K, V> {
    fn new(size_hint: Option<usize>) -> Self;

    /// Return true if the insert was successful and the key did not exist in the map
    fn insert(&mut self, key: K, value: V) -> bool;
}

impl<K, V, S> PreventDuplicateInsertsMap<K, V> for HashMap<K, V, S>
where
    K: Eq + Hash,
    S: BuildHasher + Default,
{
    #[inline]
    fn new(size_hint: Option<usize>) -> Self {
        match size_hint {
            Some(size) => Self::with_capacity_and_hasher(size, S::default()),
            None => Self::with_hasher(S::default()),
        }
    }

    #[inline]
    fn insert(&mut self, key: K, value: V) -> bool {
        self.insert(key, value).is_none()
    }
}

/// Deserialize a map and return an error on duplicate keys
pub fn deserialize<'de, D, T, K, V>(deserializer: D) -> Result<T, D::Error>
where
    T: PreventDuplicateInsertsMap<K, V>,
    K: Deserialize<'de>,
    V: Deserialize<'de>,
    D: Deserializer<'de>,
{
    struct MapVisitor<T, K, V> {
        marker: PhantomData<T>,
        map_key_type: PhantomData<K>,
        map_value_type: PhantomData<V>,
    }

    impl<'de, T, K, V> Visitor<'de> for MapVisitor<T, K, V>
    where
        T: PreventDuplicateInsertsMap<K, V>,
        K: Deserialize<'de>,
        V: Deserialize<'de>,
    {
        type Value = T;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a map")
        }

        #[inline]
        fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut values = Self::Value::new(access.size_hint());

            while let Some((key, value)) = access.next_entry()? {
                if !values.insert(key, value) {
                    return Err(Error::custom("invalid entry: found duplicate key"));
                };
            }

            Ok(values)
        }
    }

    let visitor = MapVisitor {
        marker: PhantomData,
        map_key_type: PhantomData,
        map_value_type: PhantomData,
    };
    deserializer.deserialize_map(visitor)
}

/*
/// Serialize the map with the default serializer
pub fn serialize<T, S>(value: &T, serializer: S) -> Result<S::Ok, S::Error>
where
    T: Serialize,
    S: Serializer,
{
    value.serialize(serializer)
}
*/
