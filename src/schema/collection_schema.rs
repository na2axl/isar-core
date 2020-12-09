use crate::collection::IsarCollection;
use crate::data_dbs::DataDbs;
use crate::error::{illegal_arg, Result};
use crate::index::{Index, IndexType};
use crate::object::data_type::DataType;
use crate::object::object_id::ObjectId;
use crate::object::object_info::ObjectInfo;
use crate::object::property::Property;
use crate::schema::index_schema::IndexSchema;
use crate::schema::property_schema::PropertySchema;
use hashbrown::HashSet;
use itertools::Itertools;
use rand::random;
use serde::{Deserialize, Serialize};
use std::cmp;
use std::cmp::Ordering;

#[derive(Serialize, Deserialize, Clone)]
pub struct CollectionSchema {
    pub(super) id: Option<u16>,
    pub(super) name: String,
    pub(super) properties: Vec<PropertySchema>,
    pub(super) indexes: Vec<IndexSchema>,
}

impl CollectionSchema {
    pub fn new(name: &str) -> CollectionSchema {
        CollectionSchema {
            id: None,
            name: name.to_string(),
            properties: vec![],
            indexes: vec![],
        }
    }

    pub fn add_property(&mut self, name: &str, data_type: DataType) -> Result<()> {
        if name.is_empty() {
            illegal_arg("Empty properties are not allowed")?;
        }

        if self.properties.iter().any(|f| f.name == name) {
            illegal_arg("Property already exists")?;
        }

        if let Some(previous) = self.properties.last() {
            match data_type.cmp(&previous.data_type) {
                Ordering::Equal => {
                    if name < &previous.name {
                        illegal_arg("Propertys with same type need to be ordered alphabetically")?;
                    }
                }
                Ordering::Less => illegal_arg("Propertys need to be ordered by type")?,
                Ordering::Greater => {}
            }
        }

        self.properties.push(PropertySchema {
            name: name.to_string(),
            data_type,
        });

        Ok(())
    }

    pub fn add_index(
        &mut self,
        property_names: &[&str],
        unique: bool,
        hash_value: bool,
    ) -> Result<()> {
        if property_names.is_empty() {
            illegal_arg("At least one property needs to be added to a valid index.")?;
        }

        if property_names.len() > 3 {
            illegal_arg("No more than three properties may be used as a composite index.")?;
        }

        let duplicate = self.indexes.iter().any(|i| {
            let min_len = cmp::min(i.property_names.len(), property_names.len());
            i.property_names[..min_len] == property_names[..min_len]
        });
        if duplicate {
            illegal_arg("Index already exists.")?;
        }

        let properties: Option<Vec<_>> = property_names
            .iter()
            .map(|index_property| self.properties.iter().find(|p| p.name == *index_property))
            .collect();

        if properties.is_none() {
            illegal_arg("Index property does not exist.")?;
        }
        let properties = properties.unwrap();

        let illegal_data_type = properties
            .iter()
            .any(|p| p.data_type.is_dynamic() && p.data_type != DataType::String);
        if illegal_data_type {
            illegal_arg("Illegal index data type.")?;
        }

        let has_string_properties = properties.iter().any(|p| p.data_type == DataType::String);
        if !has_string_properties && hash_value {
            illegal_arg("Only string indexes can be hashed.")?;
        }

        if !hash_value {
            for (index, property) in properties.iter().enumerate() {
                if property.data_type == DataType::String && index < properties.len() - 1 {
                    illegal_arg(
                        "Non-hashed string indexes must only be at the end of a composite index.",
                    )?;
                }
            }
        }

        self.indexes
            .push(IndexSchema::new(property_names, unique, hash_value));

        Ok(())
    }

    pub(super) fn get_isar_collection(&self, dbs: DataDbs) -> IsarCollection {
        let properties = self.get_properties();
        let indexes = self.get_indexes(&properties, dbs);
        let object_info = ObjectInfo::new(properties);
        IsarCollection::new(self.id.unwrap(), object_info, indexes, dbs.primary)
    }

    fn get_properties(&self) -> Vec<Property> {
        let oid_offset = ObjectId::get_size();
        let mut offset = oid_offset;

        self.properties
            .iter()
            .map(|f| {
                let size = f.data_type.get_static_size();

                if offset % size != 0 {
                    offset += size - offset % size;
                }
                // padding to align data
                let property = Property::new(f.data_type, offset - oid_offset);
                offset += size;

                property
            })
            .collect()
    }

    fn get_indexes(&self, properties: &[Property], dbs: DataDbs) -> Vec<Index> {
        self.indexes
            .iter()
            .map(|index| {
                let properties = index
                    .property_names
                    .iter()
                    .map(|name| {
                        let pos = self
                            .properties
                            .iter()
                            .position(|property| &property.name == name)
                            .unwrap();
                        properties.get(pos).unwrap()
                    })
                    .cloned()
                    .collect_vec();
                let (index_type, db) = if index.unique {
                    (IndexType::Secondary, dbs.secondary)
                } else {
                    (IndexType::SecondaryDup, dbs.secondary_dup)
                };
                Index::new(
                    index.id.unwrap(),
                    properties,
                    index_type,
                    index.hash_value,
                    db,
                )
            })
            .collect()
    }

    pub(super) fn update_with_existing_collection(
        &mut self,
        existing_collections: &[CollectionSchema],
        used_ids: &mut HashSet<u16>,
    ) {
        if let Some(existing_collection) = existing_collections.iter().find(|c| c.name == self.name)
        {
            let properties = &self.properties;

            for index in &mut self.indexes {
                let existing_index = existing_collection.indexes.iter().find(|i| &index == i);
                if let Some(existing_index) = existing_index {
                    let can_reuse_index = !index.property_names.iter().any(|property_name| {
                        let property = properties
                            .iter()
                            .find(|f| &f.name == property_name)
                            .unwrap();
                        let existing_property = existing_collection
                            .properties
                            .iter()
                            .find(|f| &f.name == property_name)
                            .unwrap();
                        property.data_type != existing_property.data_type
                    });

                    if can_reuse_index {
                        index.id = existing_index.id;
                    }
                }
            }

            if self.properties == existing_collection.properties
                && self.indexes == existing_collection.indexes
            {
                self.id = existing_collection.id;
            }
        }

        if self.id.is_none() {
            self.id = Some(Self::find_id(used_ids));
        }
        for index in &mut self.indexes {
            if index.id.is_none() {
                index.id = Some(Self::find_id(used_ids));
            }
        }
    }

    fn find_id(used_ids: &mut HashSet<u16>) -> u16 {
        loop {
            let id = random();
            if used_ids.insert(id) {
                return id;
            }
        }
    }

    pub(super) fn collect_ids(&self, ids: &mut HashSet<u16>) {
        if let Some(id) = self.id {
            ids.insert(id);
        }
        for index in &self.indexes {
            if let Some(id) = index.id {
                assert!(
                    ids.insert(id),
                    "Something is wrong, schema contains duplicate id."
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lmdb::db::Db;

    #[test]
    fn test_add_property_empty_name() {
        let mut col = CollectionSchema::new("col");
        assert!(col.add_property("", DataType::Int).is_err())
    }

    #[test]
    fn test_add_property_duplicate_name() {
        let mut col = CollectionSchema::new("col");
        col.add_property("prop", DataType::Int).unwrap();
        assert!(col.add_property("prop", DataType::Int).is_err())
    }

    #[test]
    fn test_add_property_same_type_wrong_order() {
        let mut col = CollectionSchema::new("col");

        col.add_property("b", DataType::Int).unwrap();
        assert!(col.add_property("a", DataType::Int).is_err())
    }

    #[test]
    fn test_add_property_wrong_order() {
        let mut col = CollectionSchema::new("col");

        col.add_property("a", DataType::Long).unwrap();
        assert!(col.add_property("b", DataType::Int).is_err())
    }

    #[test]
    fn test_add_index_without_properties() {
        let mut col = CollectionSchema::new("col");

        assert!(col.add_index(&[], false, false).is_err())
    }

    #[test]
    fn test_add_index_with_non_existing_property() {
        let mut col = CollectionSchema::new("col");
        col.add_property("prop1", DataType::Int).unwrap();

        col.add_index(&["prop1"], false, false).unwrap();
        assert!(col.add_index(&["wrongprop"], false, false).is_err())
    }

    #[test]
    fn test_add_index_with_illegal_data_type() {
        let mut col = CollectionSchema::new("col");
        col.add_property("bool", DataType::Bool).unwrap();
        col.add_property("int", DataType::Int).unwrap();
        col.add_property("float", DataType::Float).unwrap();
        col.add_property("long", DataType::Long).unwrap();
        col.add_property("double", DataType::Double).unwrap();
        col.add_property("str", DataType::String).unwrap();
        col.add_property("bytes", DataType::Bytes).unwrap();
        col.add_property("intlist", DataType::IntList).unwrap();

        col.add_index(&["int"], false, false).unwrap();
        col.add_index(&["long"], false, false).unwrap();
        col.add_index(&["float"], false, false).unwrap();
        col.add_index(&["double"], false, false).unwrap();
        col.add_index(&["bool"], false, false).unwrap();
        col.add_index(&["str"], false, false).unwrap();
        assert!(col.add_index(&["bytes"], false, false).is_err());
        assert!(col.add_index(&["intlist"], false, false).is_err());
    }

    #[test]
    fn test_add_index_too_many_properties() {
        let mut col = CollectionSchema::new("col");
        col.add_property("prop1", DataType::Int).unwrap();
        col.add_property("prop2", DataType::Int).unwrap();
        col.add_property("prop3", DataType::Int).unwrap();
        col.add_property("prop4", DataType::Int).unwrap();

        assert!(col
            .add_index(&["prop1", "prop2", "prop3", "prop4"], false, false)
            .is_err())
    }

    #[test]
    fn test_add_duplicate_index() {
        let mut col = CollectionSchema::new("col");
        col.add_property("prop1", DataType::Int).unwrap();
        col.add_property("prop2", DataType::Int).unwrap();

        col.add_index(&["prop2"], false, false).unwrap();
        col.add_index(&["prop1", "prop2"], false, false).unwrap();
        assert!(col.add_index(&["prop1", "prop2"], false, false).is_err());
        assert!(col.add_index(&["prop1"], false, false).is_err());
    }

    #[test]
    fn test_add_composite_index_with_non_hashed_string_in_the_middle() {
        let mut col = CollectionSchema::new("col");
        col.add_property("int", DataType::Int).unwrap();
        col.add_property("str", DataType::String).unwrap();

        col.add_index(&["int", "str"], false, false).unwrap();
        assert!(col.add_index(&["str", "int"], false, false).is_err());
        col.add_index(&["str", "int"], false, true).unwrap();
    }

    #[test]
    fn test_properties_have_correct_offset() {
        fn get_offsets(mut schema: CollectionSchema) -> Vec<usize> {
            let mut ids = HashSet::new();
            schema.update_with_existing_collection(&[], &mut ids);
            let col = schema.get_isar_collection(DataDbs::debug_new());
            let mut offsets = vec![];
            for i in 0..schema.properties.len() {
                offsets.push(col.get_property_by_index(i).unwrap().offset);
            }
            offsets
        }

        let mut col = CollectionSchema::new("col");
        col.add_property("bool", DataType::Bool).unwrap();
        col.add_property("int", DataType::Int).unwrap();
        col.add_property("double", DataType::Double).unwrap();
        assert_eq!(get_offsets(col), vec![0, 2, 10]);

        let mut col = CollectionSchema::new("col");
        col.add_property("bool1", DataType::Bool).unwrap();
        col.add_property("bool2", DataType::Bool).unwrap();
        col.add_property("bool3", DataType::Bool).unwrap();
        col.add_property("str", DataType::String).unwrap();
        assert_eq!(get_offsets(col), vec![0, 1, 2, 10]);

        let mut col = CollectionSchema::new("col");
        col.add_property("bytes", DataType::Bytes).unwrap();
        col.add_property("intList", DataType::IntList).unwrap();
        col.add_property("doubleList", DataType::DoubleList)
            .unwrap();
        assert_eq!(get_offsets(col), vec![2, 10, 18]);
    }
}
