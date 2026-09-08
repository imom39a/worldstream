use super::{add_seconds, subtract_seconds, successor};
use crate::TimerScheduledFor;

#[test]
fn every_fraction_width_produces_canonical_phase_deadlines() -> anyhow::Result<()> {
    for fraction in [
        "",
        ".1",
        ".12",
        ".123",
        ".1234",
        ".12345",
        ".123456",
        ".1234567",
        ".12345678",
        ".123456789",
        ".914230084",
        ".100000001",
        ".000000001",
    ] {
        let input = format!("2026-09-08T03:37:27{fraction}Z");
        for seconds in [1, 10, 20, 30, 90] {
            let value = add_seconds(&input, seconds)?;
            assert!(
                value.parse::<TimerScheduledFor>().is_ok(),
                "{input} -> {value}"
            );
            let reminder = subtract_seconds(&value, 10)?;
            assert!(reminder.parse::<TimerScheduledFor>().is_ok(), "{reminder}");
        }
    }
    Ok(())
}

#[test]
fn successor_and_rollover_keep_minimal_fractional_precision() -> anyhow::Result<()> {
    for (input, expected) in [
        ("2026-09-08T03:37:27.123459Z", "2026-09-08T03:37:27.12346Z"),
        ("2026-12-31T23:59:59.999999Z", "2027-01-01T00:00:00Z"),
        ("2026-09-08T03:37:27.099999Z", "2026-09-08T03:37:27.1Z"),
    ] {
        let value = successor(input)?;
        assert_eq!(value, expected);
        assert!(value.parse::<TimerScheduledFor>().is_ok());
    }
    Ok(())
}
