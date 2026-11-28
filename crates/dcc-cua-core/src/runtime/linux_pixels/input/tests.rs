use super::*;
use rstest::rstest;

#[ignore = "requires a private Xvfb server"]
#[rstest]
fn linux_pixels_native_expired_fence_releases_its_grab() {
    let (owner, _) = transport::connect().unwrap();
    let owner = Arc::new(owner);
    let (peer, _) = transport::connect().unwrap();
    let fence = DispatchFence::acquire(&owner).unwrap();
    std::thread::sleep(transport::OPERATION_TIMEOUT + Duration::from_millis(10));
    drop(fence);
    peer.stream().renew();
    peer.get_input_focus().unwrap().reply().unwrap();
    owner.stream().renew();
    owner.get_input_focus().unwrap().reply().unwrap();
}
