use super::*;
use crate::async_runtime::{Flavor, flavor};
use rstest::rstest;

#[rstest]
fn mcp_server_uses_the_small_single_thread_runtime() {
    assert_eq!(flavor(&strings(["mcp-server"])), Flavor::CurrentThread);
    assert_eq!(flavor(&strings(["host"])), Flavor::MultiThread);
    assert_eq!(
        flavor(&strings(["snapshot", "--pid", "42"])),
        Flavor::MultiThread
    );
}
