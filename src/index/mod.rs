use crate::error::{IsarError, Result};
use crate::index::index_key::IndexKey;
use crate::lmdb::{ByteKey, IntKey, Key};
use crate::object::data_type::DataType;
use crate::object::isar_object::{IsarObject, Property};
use crate::query::index_where_clause::IndexWhereClause;
use crate::query::Sort;
use crate::schema::collection_schema::IndexType;
use crate::txn::{Cursors, IsarTxn};
use crate::utils::debug::dump_db;
use hashbrown::HashSet;
use itertools::Itertools;
use unicode_segmentation::UnicodeSegmentation;

pub mod index_key;

pub const MAX_STRING_INDEX_SIZE: usize = 1024;

/*

Null values are always considered the "smallest" element.

 */

#[derive(Copy, Clone, Eq, PartialEq)]
pub struct IndexProperty {
    pub property: Property,
    pub index_type: IndexType,
    pub case_sensitive: Option<bool>,
}

impl IndexProperty {
    pub(crate) fn new(
        property: Property,
        index_type: IndexType,
        case_sensitive: Option<bool>,
    ) -> Self {
        IndexProperty {
            property,
            index_type,
            case_sensitive,
        }
    }

    pub fn get_string_with_case(&self, object: IsarObject) -> Option<String> {
        object.read_string(self.property).map(|str| {
            if self.case_sensitive.unwrap() {
                str.to_string()
            } else {
                str.to_lowercase()
            }
        })
    }
}

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct Index {
    pub id: u16,
    pub col_id: u16,
    pub properties: Vec<IndexProperty>,
    pub unique: bool,
    pub replace: bool,
}

impl Index {
    pub fn new(
        id: u16,
        col_id: u16,
        properties: Vec<IndexProperty>,
        unique: bool,
        replace: bool,
    ) -> Self {
        Index {
            id,
            col_id,
            properties,
            unique,
            replace,
        }
    }

    pub fn get_prefix(&self) -> Vec<u8> {
        self.id.to_be_bytes().to_vec()
    }

    pub fn create_for_object<F>(
        &self,
        cursors: &mut Cursors,
        oid: i64,
        object: IsarObject,
        mut delete_existing: F,
    ) -> Result<()>
    where
        F: FnMut(&mut Cursors, i64) -> Result<()>,
    {
        let id_key = IntKey::new(self.col_id, oid);
        self.create_keys(object, |key| {
            self.create_for_object_key(cursors, id_key, ByteKey::new(key), &mut delete_existing)?;
            Ok(true)
        })
    }

    fn create_for_object_key<F>(
        &self,
        cursors: &mut Cursors,
        id_key: IntKey,
        key: ByteKey,
        mut delete_existing: F,
    ) -> Result<()>
    where
        F: FnMut(&mut Cursors, i64) -> Result<()>,
    {
        if self.unique {
            let success = cursors.index.put_no_override(key, id_key.as_bytes())?;
            if !success {
                if self.replace {
                    delete_existing(cursors, id_key.get_id())?;
                } else {
                    return Err(IsarError::UniqueViolated {});
                }
            }
        } else {
            cursors.index.put(key, id_key.as_bytes())?;
        }
        Ok(())
    }

    pub fn delete_for_object(
        &self,
        cursors: &mut Cursors,
        oid: i64,
        object: IsarObject,
    ) -> Result<()> {
        let key = IntKey::new(self.col_id, oid);
        let oid_bytes = key.as_bytes();
        self.create_keys(object, |key| {
            let entry = cursors
                .index
                .move_to_key_val(ByteKey::new(key), oid_bytes)?;
            if entry.is_some() {
                cursors.index.delete_current()?;
            }
            Ok(true)
        })
    }

    pub fn clear(&self, cursors: &mut Cursors) -> Result<()> {
        IndexWhereClause::new(
            IndexKey::new(self),
            IndexKey::new(self),
            false,
            Sort::Ascending,
        )?
        .iter_ids(&mut cursors.index, |cursor, _| {
            cursor.delete_current()?;
            Ok(true)
        })?;
        Ok(())
    }

    pub fn create_keys(
        &self,
        object: IsarObject,
        mut callback: impl FnMut(&[u8]) -> Result<bool>,
    ) -> Result<()> {
        let mut key = IndexKey::new(self);
        Self::fill_single_key(&mut key, &self.properties, object);

        let last_property = self.properties.last().unwrap();
        if last_property.index_type == IndexType::Words {
            let mut result = Ok(());
            Self::fill_word_keys(&mut key, *last_property, object, |bytes| {
                match callback(bytes) {
                    Ok(cont) => cont,
                    Err(err) => {
                        result = Err(err);
                        false
                    }
                }
            });
            result
        } else {
            callback(&key.bytes)?;
            Ok(())
        }
    }

    fn fill_single_key(key: &mut IndexKey, properties: &[IndexProperty], object: IsarObject) {
        for ip in properties {
            match ip.property.data_type {
                DataType::Byte => {
                    let value = object.read_byte(ip.property);
                    key.add_byte(value);
                }
                DataType::Int => {
                    let value = object.read_int(ip.property);
                    key.add_int(value);
                }
                DataType::Long => {
                    let value = object.read_long(ip.property);
                    key.add_long(value);
                }
                DataType::Float => {
                    let value = object.read_float(ip.property);
                    key.add_float(value);
                }
                DataType::Double => {
                    let value = object.read_double(ip.property);
                    key.add_double(value);
                }
                DataType::String => {
                    let value = object.read_string(ip.property);
                    match ip.index_type {
                        IndexType::Value => key.add_string_value(value, ip.case_sensitive.unwrap()),
                        IndexType::Hash => key.add_string_hash(value, ip.case_sensitive.unwrap()),
                        _ => {}
                    }
                }
                _ => unimplemented!(),
            }
        }
    }

    fn fill_word_keys(
        key: &mut IndexKey,
        property: IndexProperty,
        object: IsarObject,
        mut callback: impl FnMut(&[u8]) -> bool,
    ) {
        let key_len = key.len();
        let value = property.get_string_with_case(object);
        if let Some(str) = value {
            for word in str.unicode_words().unique() {
                key.truncate(key_len);
                key.add_string_word(word, property.case_sensitive.unwrap());
                if !callback(&key.bytes) {
                    break;
                }
            }
        }
    }

    pub fn debug_dump(&self, txn: &mut IsarTxn) -> HashSet<(Vec<u8>, Vec<u8>)> {
        txn.read(|cursors| {
            let set = dump_db(&mut cursors.index, Some(&self.id.to_be_bytes()))
                .into_iter()
                .map(|(key, val)| (key.to_vec(), val.to_vec()))
                .collect();
            Ok(set)
        })
        .unwrap()
    }

    pub fn debug_create_keys(&self, object: IsarObject) -> Vec<Vec<u8>> {
        let mut keys = vec![];
        self.create_keys(object, |key| {
            keys.push(key.to_vec());
            Ok(true)
        })
        .unwrap();
        keys
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collection::IsarCollection;
    use crate::instance::IsarInstance;
    use crate::object::data_type::DataType;
    use crate::{col, ind, isar};

    fn check_index(isar: &IsarInstance, col: &IsarCollection, obj: IsarObject) {
        let mut txn = isar.begin_txn(true, false).unwrap();
        let oid = obj.read_id();
        col.put(&mut txn, obj).unwrap();
        let index = col.debug_get_indexes().get(0).unwrap();

        let set: HashSet<(Vec<u8>, Vec<u8>)> = index
            .debug_create_keys(obj)
            .into_iter()
            .map(|key| (key, IntKey::new(col.id, oid).as_bytes().to_vec()))
            .collect();

        assert_eq!(index.debug_dump(&mut txn), set)
    }

    #[test]
    fn test_create_for_object_byte() {
        isar!(isar, col => col!(field => DataType::Byte; ind!(field)));
        let mut builder = col.new_object_builder(None);
        builder.write_long(1);
        builder.write_byte(123);
        check_index(&isar, col, builder.finish());
        isar.close();
    }

    #[test]
    fn test_create_for_object_int() {
        isar!(isar, col => col!(field => DataType::Int; ind!(field)));
        let mut builder = col.new_object_builder(None);
        builder.write_long(1);
        builder.write_int(123);
        check_index(&isar, col, builder.finish());
        isar.close();
    }

    #[test]
    fn test_create_for_object_float() {
        isar!(isar, col => col!(field => DataType::Float; ind!(field)));
        let mut builder = col.new_object_builder(None);
        builder.write_long(1);
        builder.write_float(123.321);
        check_index(&isar, col, builder.finish());
        isar.close();
    }

    #[test]
    fn test_create_for_object_long() {
        isar!(isar, col => col!(field => DataType::Long; ind!(field)));
        let mut builder = col.new_object_builder(None);
        builder.write_long(1);
        builder.write_long(123321);
        check_index(&isar, col, builder.finish());
        isar.close();
    }

    #[test]
    fn test_create_for_object_double() {
        isar!(isar, col => col!(field => DataType::Double; ind!(field)));
        let mut builder = col.new_object_builder(None);
        builder.write_long(1);
        builder.write_double(123123.321321);
        check_index(&isar, col, builder.finish());
        isar.close();
    }

    #[test]
    fn test_create_for_object_string() {
        fn test(str_type: IndexType, str_lc: bool) {
            isar!(isar, col => col!(field => DataType::String; ind!(str field, str_type, Some(str_lc))));
            let mut builder = col.new_object_builder(None);
            builder.write_long(1);
            builder.write_string(Some("Hello This Is A TEST Hello"));
            check_index(&isar, col, builder.finish());
            isar.close();
        }

        for str_type in &[IndexType::Words] {
            test(*str_type, false);
            //test(*str_type, true);
        }
    }

    #[test]
    fn test_create_for_object_unique() {}

    #[test]
    fn test_create_for_object_violate_unique() {
        isar!(isar, col => col!(field => DataType::Int; ind!(field; true, false)));
        let mut txn = isar.begin_txn(true, false).unwrap();

        let mut ob = col.new_object_builder(None);
        ob.write_long(1);
        ob.write_int(5);
        col.put(&mut txn, ob.finish()).unwrap();

        let mut ob = col.new_object_builder(None);
        ob.write_long(2);
        ob.write_int(5);
        let result = col.put(&mut txn, ob.finish());
        match result {
            Err(IsarError::UniqueViolated { .. }) => {}
            _ => panic!("wrong error"),
        };
        txn.abort();
        isar.close();
    }

    #[test]
    fn test_create_for_object_compound() {}

    #[test]
    fn test_delete_for_object() {}

    #[test]
    fn test_clear() {}

    #[test]
    fn test_create_key() {}
}
