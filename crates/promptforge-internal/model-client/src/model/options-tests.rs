//! Tests for the model set's read view and the temperature range: the
//! poisoned and healthy view methods, and the finite range check.

use super::*;

/// A model set whose mutex a panicking thread poisoned.
fn poisoned_set() -> Mutex<ModelSet> {
    let set = Mutex::new(ModelSet::default());
    std::thread::scope(|scope| {
        let _ = scope
            .spawn(|| {
                let _guard = set.lock();
                panic!("poison the set");
            })
            .join();
    });
    set
}

#[test]
fn a_poisoned_model_set_reports_the_lock_error_from_every_view_method() {
    let set = poisoned_set();
    assert_eq!(set.bindings(), Err(ModelSetError));
    assert_eq!(ModelView::default(&set), Err(ModelSetError));
    assert_eq!(set.binding("writer"), Err(ModelSetError));
    assert_eq!(ModelSetError.to_string(), "model set mutex was poisoned");
}

#[test]
fn a_healthy_model_set_answers_every_view_method() {
    let set = Mutex::new(ModelSet::default());
    assert_eq!(set.bindings(), Ok(Vec::new()));
    assert_eq!(ModelView::default(&set), Ok(None));
    assert_eq!(set.binding("writer"), Ok(None));
}

#[test]
fn a_model_set_compares_as_a_total_equivalence() {
    fn assert_eq_impl<T: Eq>() {}
    assert_eq_impl::<Temperature>();
    assert_eq_impl::<ModelInvocation>();
    assert_eq_impl::<ModelBinding>();
    assert_eq_impl::<ModelSet>();
}

#[test]
fn with_temperature_rejects_non_finite_and_out_of_range() {
    let base = || CompletionOptions::new("m");
    assert_eq!(
        base().with_temperature(f64::NAN),
        Err(TemperatureError::NotFinite)
    );
    assert_eq!(
        base().with_temperature(f64::INFINITY),
        Err(TemperatureError::NotFinite)
    );
    assert!(matches!(
        base().with_temperature(-0.1),
        Err(TemperatureError::OutOfRange { .. })
    ));
    assert!(matches!(
        base().with_temperature(2.5),
        Err(TemperatureError::OutOfRange { .. })
    ));
    // The range endpoints and an interior value are accepted.
    assert_eq!(
        base()
            .with_temperature(0.0)
            .expect("0.0 is valid")
            .temperature
            .map(Temperature::get),
        Some(0.0)
    );
    assert_eq!(
        base()
            .with_temperature(TEMPERATURE_MAX)
            .expect("2.0 is valid")
            .temperature
            .map(Temperature::get),
        Some(TEMPERATURE_MAX)
    );
    assert_eq!(
        base()
            .with_temperature(0.7)
            .expect("0.7 is valid")
            .temperature
            .map(Temperature::get),
        Some(0.7)
    );
}
