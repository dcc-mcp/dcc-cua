use super::*;
use rstest::rstest;

#[rstest]
#[case(false)]
#[case(true)]
fn owned_socket_cleanup_preserves_replaced_paths(#[case] replace: bool) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("owned.sock");
    let _listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    let metadata = std::fs::symlink_metadata(&path).unwrap();
    let owned = OwnedSocket {
        path: path.clone(),
        device: metadata.dev(),
        inode: metadata.ino(),
    };
    if replace {
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"replacement").unwrap();
    }
    drop(owned);
    assert_eq!(path.exists(), replace);
    if replace {
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
    }
}
