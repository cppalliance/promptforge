//! The shared ordering of a Lua table's hash keys.
//!
//! Lua leaves a table's hash traversal order unspecified, so every path that
//! turns a table into an ordered sequence imposes the same order: booleans
//! first (`false` before `true`), then numbers by value, then strings
//! bytewise. [`sort_key`] classifies one Lua key into its [`SortKey`];
//! [`SortKey::compare`] orders two of them. Fanout's member enumeration and
//! the deterministic `pairs`/`next` installer share both, so a table's order
//! is a function of its contents wherever it is read.

use std::cmp::Ordering;

use mlua::Value;

/// A hash key's sort position: booleans first (`false` before `true`), then
/// numbers by value, then strings bytewise. The ranks keep mixed-type keys
/// totally ordered without inventing a cross-type comparison. An integer
/// key stays an `i64` so two distinct integers past 2^53 never compare
/// equal (which would leave their order to `pairs`, the nondeterminism the
/// sort exists to remove); only a mixed integer/float pair converts.
#[derive(Debug)]
pub(crate) enum SortKey {
    Bool(bool),
    Integer(i64),
    Float(f64),
    Text(Vec<u8>),
}

impl SortKey {
    /// The type's ordering rank: booleans, then numbers, then strings.
    fn rank(&self) -> u8 {
        match self {
            SortKey::Bool(_) => 0,
            SortKey::Integer(_) | SortKey::Float(_) => 1,
            SortKey::Text(_) => 2,
        }
    }

    /// Orders two positions of any type: same types compare within their
    /// type, a mixed integer and float compares exactly, and a mixed type
    /// falls back to its rank.
    pub(crate) fn compare(&self, other: &SortKey) -> Ordering {
        match (self, other) {
            (SortKey::Bool(left), SortKey::Bool(right)) => left.cmp(right),
            (SortKey::Integer(left), SortKey::Integer(right)) => left.cmp(right),
            (SortKey::Float(left), SortKey::Float(right)) => left.total_cmp(right),
            (SortKey::Integer(integer), SortKey::Float(float)) => {
                compare_integer_float(*integer, *float)
            }
            (SortKey::Float(float), SortKey::Integer(integer)) => {
                compare_integer_float(*integer, *float).reverse()
            }
            (SortKey::Text(left), SortKey::Text(right)) => left.cmp(right),
            _ => self.rank().cmp(&other.rank()),
        }
    }
}

/// Orders an integer key against a finite float key exactly: the float is
/// compared to the integer's neighborhood without rounding the integer,
/// so an integer past 2^53 still sorts on the correct side of a nearby
/// float. A float outside `i64`'s range is beyond every integer; a float
/// inside it is truncated, the integer parts are compared, and a tie is
/// broken by the float's fractional part (an exact integer-valued float
/// ties with its integer).
fn compare_integer_float(integer: i64, float: f64) -> Ordering {
    /// 2^63: one past `i64::MAX`, exactly representable, so a float at or
    /// beyond it is greater than every integer.
    const ABOVE_MAX: f64 = 9_223_372_036_854_775_808.0;
    /// -2^63: exactly `i64::MIN`, so a float below it is less than every
    /// integer.
    const MIN: f64 = -9_223_372_036_854_775_808.0;
    if float >= ABOVE_MAX {
        return Ordering::Less;
    }
    if float < MIN {
        return Ordering::Greater;
    }
    // In range and finite: the truncation is exact for the integer part.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the float is inside i64's range and its fractional part is compared separately"
    )]
    let truncated = float.trunc() as i64;
    match integer.cmp(&truncated) {
        Ordering::Equal => {
            // The integer equals the float's integer part, so it sits below
            // a float with a positive fraction and above one with a
            // negative fraction.
            let fraction = float - float.trunc();
            if fraction > 0.0 {
                Ordering::Less
            } else if fraction < 0.0 {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        }
        ordering => ordering,
    }
}

/// Why a Lua value cannot be a sortable hash key.
#[derive(Debug)]
pub(crate) enum KeyError {
    /// A number key that is NaN or infinite: it has no ordered position.
    NotFinite,
    /// A string key whose bytes are not valid UTF-8, so it has no label.
    NotUtf8(mlua::Error),
    /// A key whose Lua type is not string, number, or boolean.
    Unsortable(&'static str),
}

/// Classifies a Lua value as a sortable hash key: its sort position plus the
/// label naming it (a string's text, any other scalar's rendering).
///
/// # Errors
/// Returns [`KeyError`] when the value has no ordered position: a non-scalar
/// key, a string whose bytes are not valid UTF-8, or a number that is not
/// finite. Each caller maps the failure onto its own diagnostic, so this
/// helper names no feature.
pub(crate) fn sort_key(key: &Value) -> Result<(SortKey, String), KeyError> {
    match key {
        Value::String(s) => {
            let label = s.to_str().map_err(KeyError::NotUtf8)?.to_owned();
            Ok((SortKey::Text(s.as_bytes().to_vec()), label))
        }
        Value::Integer(i) => Ok((SortKey::Integer(*i), i.to_string())),
        Value::Number(n) => {
            if n.is_finite() {
                Ok((SortKey::Float(*n), n.to_string()))
            } else {
                Err(KeyError::NotFinite)
            }
        }
        Value::Boolean(b) => Ok((SortKey::Bool(*b), b.to_string())),
        other => Err(KeyError::Unsortable(other.type_name())),
    }
}

#[cfg(test)]
#[path = "collection-order-tests.rs"]
mod tests;
