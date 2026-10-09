use rstest::rstest;

use super::*;

#[rstest]
fn trusted_lease_deadline_is_fixed_before_slow_identity_reads() {
    let deadline = trusted_lease_deadline(500, 12_000, 10_000).unwrap();
    assert_eq!(deadline, 2500);
    assert_eq!(deadline.saturating_sub(2300), 200);
}

#[rstest]
#[case(10_000)]
#[case(9_999)]
#[case(0)]
fn expired_trusted_lease_cannot_begin_preparation(#[case] expires: u64) {
    assert_eq!(
        trusted_lease_deadline(500, expires, 10_000)
            .unwrap_err()
            .reason,
        PreparationFailure::Expired
    );
}

#[rstest]
fn trusted_lease_deadline_overflow_is_rejected() {
    assert_eq!(
        trusted_lease_deadline(u64::MAX, 10_001, 10_000)
            .unwrap_err()
            .reason,
        PreparationFailure::InvalidBinding
    );
}
