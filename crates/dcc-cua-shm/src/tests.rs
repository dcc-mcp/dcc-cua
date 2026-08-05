use rstest::rstest;

use super::*;

#[rstest]
fn descriptor_uses_cua_shared_memory_shape() {
    let image = SharedImage::from_bytes(b"png", "image/png").unwrap();
    let descriptor = image.descriptor();
    assert!(descriptor.name.starts_with("cua_"));
    assert_eq!(descriptor.length, 3);
    assert_eq!(descriptor.mime_type, "image/png");
    assert!(image.is_alive());

    let opened = ipckit::SharedMemory::open(&descriptor.name).unwrap();
    let header = opened.read(0, HEADER_SIZE).unwrap();
    assert_eq!(
        u64::from_ne_bytes(header[0..8].try_into().unwrap()),
        HEADER_MAGIC
    );
    assert_eq!(u64::from_ne_bytes(header[8..16].try_into().unwrap()), 3);
    assert_eq!(&opened.read(HEADER_SIZE, 3).unwrap(), b"png");
}

#[rstest]
fn reader_opens_and_reads_host_owned_image() {
    let image = SharedImage::from_bytes(b"png", "image/png").unwrap();
    let reader = SharedImageReader::open(image.descriptor().clone()).unwrap();
    assert_eq!(reader.read().unwrap(), b"png");
    assert_eq!(reader.descriptor().mime_type, "image/png");
}

#[rstest]
fn image_size_is_bounded() {
    assert!(matches!(
        SharedImage::from_bytes(&vec![0; MAX_IMAGE_BYTES + 1], "image/png"),
        Err(SharedImageError::TooLarge)
    ));
}
