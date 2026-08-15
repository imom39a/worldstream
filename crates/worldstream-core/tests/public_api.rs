use std::mem::size_of;

use worldstream_core::{
    CoreReducerV1, PreparedCoreStateV1, PreparedRoomTransitionV1, RoomTransitionPreparerV1,
    RoomTransitionStateV1, TimerRequestV1, VerifiedCoreStateV1,
};

#[test]
fn deep_reduction_and_timer_types_are_reachable_downstream() {
    assert!(size_of::<CoreReducerV1>() > 0);
    assert!(size_of::<PreparedCoreStateV1>() > 0);
    assert!(size_of::<RoomTransitionPreparerV1>() > 0);
    assert!(size_of::<RoomTransitionStateV1>() > 0);
    assert!(size_of::<PreparedRoomTransitionV1>() > 0);
    assert!(size_of::<VerifiedCoreStateV1>() > 0);
    assert!(size_of::<TimerRequestV1>() > 0);
}
