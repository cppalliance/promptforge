//! Tests for the shared hash-key ordering helper.

use std::cmp::Ordering;

use mlua::{Lua, Value};

use super::{KeyError, SortKey, compare_integer_float, sort_key};

#[test]
fn compare_integer_float_orders_without_rounding_the_integer() {
    assert_eq!(compare_integer_float(2, 2.5), Ordering::Less);
    assert_eq!(compare_integer_float(3, 2.5), Ordering::Greater);
    assert_eq!(compare_integer_float(2, 2.0), Ordering::Equal);
    assert_eq!(compare_integer_float(-1, -0.5), Ordering::Less);
    assert_eq!(compare_integer_float(0, -0.5), Ordering::Greater);
    assert_eq!(compare_integer_float(i64::MAX, 1e300), Ordering::Less);
    assert_eq!(compare_integer_float(i64::MIN, -1e300), Ordering::Greater);
    // 2^63 as a float is one past i64::MAX, so the largest integer is
    // still below it; -2^63 is exactly i64::MIN.
    assert_eq!(
        compare_integer_float(i64::MAX, 9_223_372_036_854_775_808.0),
        Ordering::Less
    );
    assert_eq!(
        compare_integer_float(i64::MIN, -9_223_372_036_854_775_808.0),
        Ordering::Equal
    );
}

#[test]
fn sort_key_classifies_scalars_and_rejects_unsortable_keys() {
    let lua = Lua::new();

    let (key, label) = sort_key(&Value::Boolean(false)).expect("a boolean classifies");
    assert!(matches!(key, SortKey::Bool(false)));
    assert_eq!(label, "false");

    // An integer stays an integer: a float classification would tie two
    // distinct integers past 2^53 and hand their order back to `pairs`.
    let (key, label) = sort_key(&Value::Integer(i64::MAX)).expect("an integer classifies");
    assert!(matches!(key, SortKey::Integer(i64::MAX)));
    assert_eq!(label, i64::MAX.to_string());

    let (key, label) = sort_key(&Value::Number(2.5)).expect("a float classifies");
    assert!(matches!(key, SortKey::Float(value) if value.total_cmp(&2.5).is_eq()));
    assert_eq!(label, "2.5");

    let text = lua.create_string("alpha").expect("a string creates");
    let (key, label) = sort_key(&Value::String(text)).expect("a string classifies");
    assert!(matches!(key, SortKey::Text(bytes) if bytes == b"alpha"));
    assert_eq!(label, "alpha");

    // A non-finite number has no ordered position, so it is rejected
    // rather than silently ordered by a NaN or infinity comparison.
    let non_finite =
        sort_key(&Value::Number(f64::INFINITY)).expect_err("a non-finite number fails");
    assert!(matches!(non_finite, KeyError::NotFinite));

    // A table key has no cross-type rank, so it is rejected rather than
    // dropped or ordered by an invented comparison.
    let table = lua.create_table().expect("a table creates");
    let unsortable = sort_key(&Value::Table(table)).expect_err("a non-scalar key fails");
    assert!(matches!(unsortable, KeyError::Unsortable("table")));
}
