//! OFFLINE Volume upload round-trip: the FACADE handle (`modal_rust::Volume`)
//! against the in-process mock backend (`modal-rust-testkit`), end-to-end
//! through the real gRPC transport on loopback — no Modal credentials, no
//! network, no object-storage PUT.
//!
//! What this proves: the typed facade resolves a V2 volume
//! (`VolumeGetOrCreate`), plans a local file/dir into the V2 block-based
//! `VolumePutFiles2` request, and converges when the mock reports no missing
//! blocks. The mock's `volume_put_files2` returns an EMPTY `missing_blocks`
//! list, so the upload completes in round 1 with zero HTTP PUTs — exercising the
//! whole request-building + commit path offline. The actual block PUT to object
//! storage is LIVE-only (it needs a presigned URL from a real server).

use std::io::Write;

use modal_rust::Volume;
use modal_rust_testkit::prelude::*;

fn tmpdir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "mr-vol-facade-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_file(path: &std::path::Path, bytes: &[u8]) {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).unwrap();
    }
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(bytes).unwrap();
}

/// A single small file uploads cleanly: the volume resolves, and the put
/// converges (mock reports no missing blocks) with the right file/byte counts.
#[tokio::test]
async fn volume_put_single_file_round_trip() {
    let mock = MockModal::start().await.expect("mock up");
    let dir = tmpdir();
    let f = dir.join("weights.bin");
    write_file(&f, b"hello world");

    let vol = Volume::from_name_at("models", mock.url())
        .await
        .expect("resolve volume");
    assert!(vol.volume_id().starts_with("vo-"));
    assert_eq!(vol.name(), "models");

    let stats = vol
        .put(&f, "w/weights.bin", false)
        .await
        .expect("volume put");
    assert_eq!(stats.files, 1);
    assert_eq!(stats.bytes, b"hello world".len() as u64);
    // Mock reports no missing blocks ⇒ nothing actually PUT to object storage.
    assert_eq!(stats.blocks_uploaded, 0);

    std::fs::remove_dir_all(&dir).ok();
}

/// A directory uploads recursively: every regular file is declared in ONE
/// `VolumePutFiles2`, and the stats reflect the full set.
#[tokio::test]
async fn volume_put_directory_round_trip() {
    let mock = MockModal::start().await.expect("mock up");
    let dir = tmpdir();
    write_file(&dir.join("a.txt"), b"aaa");
    write_file(&dir.join("sub/b.txt"), b"bbbb");

    let vol = Volume::from_name_at("dataset", mock.url())
        .await
        .expect("resolve volume");
    let stats = vol.put(&dir, "dst", false).await.expect("volume put dir");
    assert_eq!(stats.files, 2);
    assert_eq!(stats.bytes, 3 + 4);
    assert_eq!(stats.blocks_uploaded, 0);

    std::fs::remove_dir_all(&dir).ok();
}
