//! The Windows exe icon: the crate's `build.rs` compiles
//! `crates/workshop/icons/icon.ico` into `promptforge-gateway.exe` as an
//! icon resource. An `RT_ICON` resource stores each image of the `.ico`
//! byte for byte, so every image must appear verbatim in the built binary.

use std::path::Path;

/// Reads a little-endian `u16` at `offset`.
fn u16_at(bytes: &[u8], offset: usize) -> Option<u16> {
    let raw = bytes.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([raw[0], raw[1]]))
}

/// Reads a little-endian `u32` at `offset`.
fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    let raw = bytes.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

/// Walks the `ICONDIR` at the head of an `.ico` file and returns every
/// entry's image bytes. The directory is a 6-byte header (reserved,
/// type, count) followed by 16-byte entries whose last two fields are
/// the image size and its offset from the start of the file.
fn ico_images(ico: &[u8]) -> Vec<&[u8]> {
    let count = usize::from(u16_at(ico, 4).expect("the .ico has an ICONDIR header"));
    (0..count)
        .map(|index| {
            let entry = 6 + 16 * index;
            let size = u32_at(ico, entry + 8).expect("the entry has a size");
            let offset = u32_at(ico, entry + 12).expect("the entry has an offset");
            let start = usize::try_from(offset).expect("the offset fits in usize");
            let end = start + usize::try_from(size).expect("the size fits in usize");
            ico.get(start..end).expect("the entry lies inside the file")
        })
        .collect()
}

#[test]
fn the_exe_carries_every_image_of_the_program_icon() {
    let exe = std::fs::read(env!("CARGO_BIN_EXE_promptforge-gateway")).unwrap();
    let ico_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../workshop/icons/icon.ico");
    let ico = std::fs::read(&ico_path).unwrap();

    let images = ico_images(&ico);
    assert!(!images.is_empty(), "icon.ico carries at least one image");
    for (index, image) in images.iter().enumerate() {
        assert!(
            exe.windows(image.len()).any(|window| window == *image),
            "image {index} of icon.ico ({} bytes) is not embedded in the exe",
            image.len()
        );
    }
}

#[test]
fn ico_images_reads_the_directory_entries() {
    // Two entries: a 3-byte image at offset 38 and a 2-byte image at 41.
    let mut ico = vec![0, 0, 1, 0, 2, 0];
    ico.extend_from_slice(&[16, 16, 0, 0, 1, 0, 32, 0, 3, 0, 0, 0, 38, 0, 0, 0]);
    ico.extend_from_slice(&[32, 32, 0, 0, 1, 0, 32, 0, 2, 0, 0, 0, 41, 0, 0, 0]);
    ico.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD, 0xEE]);
    assert_eq!(
        ico_images(&ico),
        [&[0xAA, 0xBB, 0xCC][..], &[0xDD, 0xEE][..]]
    );
}
