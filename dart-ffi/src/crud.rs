use crate::raw_object_set::{RawObject, RawObjectSend, RawObjectSet, RawObjectSetSend};
use crate::txn::IsarDartTxn;
use crate::{BoolSend, UintSend};
use byteorder::{ByteOrder, LittleEndian};
use isar_core::collection::IsarCollection;
use isar_core::error::Result;
use isar_core::object::isar_object::IsarObject;
use isar_core::txn::IsarTxn;
use serde_json::Value;

#[no_mangle]
pub unsafe extern "C" fn isar_get(
    collection: &'static IsarCollection,
    txn: &mut IsarDartTxn,
    object: &'static mut RawObject,
) -> i32 {
    let object = RawObjectSend(object);
    isar_try_txn!(txn, move |txn| {
        let oid = object.0.get_oid();
        let result = collection.get(txn, oid)?;
        object.0.set_object(result);
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn isar_get_all(
    collection: &'static IsarCollection,
    txn: &mut IsarDartTxn,
    objects: &'static mut RawObjectSet,
) -> i32 {
    let objects = RawObjectSetSend(objects);
    isar_try_txn!(txn, move |txn| {
        for object in objects.0.get_objects() {
            let oid = object.get_oid();
            let result = collection.get(txn, oid)?;
            object.set_object(result);
        }
        Ok(())
    })
}

fn update_auto_increment(
    collection: &IsarCollection,
    txn: &mut IsarTxn,
    bytes: &mut [u8],
) -> Result<i64> {
    let isar_object = IsarObject::from_bytes(bytes);
    let oid_property = collection.get_oid_property();
    if isar_object.is_null(oid_property) {
        let oid = collection.auto_increment(txn)?;
        LittleEndian::write_i64(&mut bytes[oid_property.offset..], oid);
        Ok(oid)
    } else {
        Ok(isar_object.read_long(oid_property))
    }
}

#[no_mangle]
pub unsafe extern "C" fn isar_put(
    collection: &'static mut IsarCollection,
    txn: &mut IsarDartTxn,
    object: &'static mut RawObject,
) -> i32 {
    let object = RawObjectSend(object);
    isar_try_txn!(txn, move |txn| {
        let bytes = object.0.get_bytes();
        let auto_increment = update_auto_increment(collection, txn, bytes)?;
        collection.put(txn, IsarObject::from_bytes(bytes))?;
        object.0.set_oid(auto_increment);
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn isar_put_all(
    collection: &'static IsarCollection,
    txn: &mut IsarDartTxn,
    objects: &'static mut RawObjectSet,
) -> i32 {
    let objects = RawObjectSetSend(objects);
    isar_try_txn!(txn, move |txn| {
        for raw_obj in objects.0.get_objects() {
            let bytes = raw_obj.get_bytes();
            let auto_increment = update_auto_increment(collection, txn, bytes)?;
            collection.put(txn, IsarObject::from_bytes(bytes))?;
            raw_obj.set_oid(auto_increment)
        }
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn isar_delete(
    collection: &'static IsarCollection,
    txn: &mut IsarDartTxn,
    oid: i64,
    deleted: &'static mut bool,
) -> i32 {
    let deleted = BoolSend(deleted);
    isar_try_txn!(txn, move |txn| {
        *deleted.0 = collection.delete(txn, oid)?;
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn isar_delete_all(
    collection: &'static IsarCollection,
    txn: &mut IsarDartTxn,
    oids: *const i64,
    oids_length: u32,
    count: &'static mut u32,
) -> i32 {
    let oids = std::slice::from_raw_parts(oids, oids_length as usize);
    let count = UintSend(count);
    isar_try_txn!(txn, move |txn| {
        for oid in oids {
            if collection.delete(txn, *oid)? {
                *count.0 += 1;
            }
        }
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn isar_clear(
    collection: &'static IsarCollection,
    txn: &mut IsarDartTxn,
    count: &'static mut u32,
) -> i32 {
    let count = UintSend(count);
    isar_try_txn!(txn, move |txn| {
        *count.0 = collection.clear(txn)? as u32;
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn isar_json_import(
    collection: &'static IsarCollection,
    txn: &mut IsarDartTxn,
    json_bytes: *const u8,
    json_length: u32,
) -> i32 {
    let bytes = std::slice::from_raw_parts(json_bytes, json_length as usize);
    let json: Value = serde_json::from_slice(bytes).unwrap();
    isar_try_txn!(txn, move |txn| { collection.import_json(txn, json) })
}
