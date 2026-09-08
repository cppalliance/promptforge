use super::sample_millis;

#[test]
fn sample_milliseconds_are_exact_across_the_prior_multiplication_overflow() {
    assert_eq!(sample_millis(18_446_744_073_709_550), 1_152_921_504_606_846);
    assert_eq!(sample_millis(18_446_744_073_709_551), 1_152_921_504_606_846);
    assert_eq!(sample_millis(18_446_744_073_709_552), 1_152_921_504_606_847);
    assert_eq!(sample_millis(18_446_744_073_709_553), 1_152_921_504_606_847);
    assert_eq!(sample_millis(u64::MAX), 1_152_921_504_606_846_975);
}
