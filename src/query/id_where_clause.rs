use crate::lmdb::cursor::Cursor;
use crate::lmdb::IntKey;
use crate::object::isar_object::IsarObject;
use crate::query::Sort;
use crate::{collection::IsarCollection, error::Result};
use hashbrown::HashSet;

#[derive(Clone)]
pub(crate) struct IdWhereClause {
    prefix: u16,
    lower: i64,
    upper: i64,
    sort: Sort,
}

impl IdWhereClause {
    pub(crate) fn new(col: &IsarCollection, lower: i64, upper: i64, sort: Sort) -> Self {
        IdWhereClause {
            prefix: col.id,
            lower,
            upper,
            sort,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.upper < self.lower
    }

    pub(crate) fn id_matches(&self, oid: i64) -> bool {
        self.lower <= oid && self.upper >= oid
    }

    pub(crate) fn iter<'txn, F>(
        &self,
        data: &mut Cursor<'txn>,
        mut result_ids: Option<&mut HashSet<i64>>,
        mut callback: F,
    ) -> Result<bool>
    where
        F: FnMut(&mut Cursor<'txn>, IntKey, IsarObject<'txn>) -> Result<bool>,
    {
        data.iter_between(
            IntKey::new(self.prefix, self.lower),
            IntKey::new(self.prefix, self.upper),
            false,
            self.sort == Sort::Ascending,
            |cursor, id, object| {
                let id = IntKey::from_bytes(id);
                if let Some(result_ids) = result_ids.as_deref_mut() {
                    if !result_ids.insert(id.get_id()) {
                        return Ok(true);
                    }
                }
                let object = IsarObject::from_bytes(object);
                callback(cursor, id, object)
            },
        )
    }

    pub(crate) fn is_overlapping(&self, other: &Self) -> bool {
        self.prefix == other.prefix
            && ((self.lower <= other.lower && self.upper >= other.upper)
                || (other.lower <= self.lower && other.upper >= self.upper))
    }
}
