use parse_load_harness::corpus::shape::Shape;
use parse_load_harness::corpus::synthetic::SyntheticCorpus;
use parse_load_harness::corpus::Corpus;

#[tokio::test]
async fn fixed_seed_produces_byte_identical_bundles() {
    let shape = Shape::load_default().unwrap();
    let mut a = SyntheticCorpus::new(shape.clone(), 0xC0FFEE).unwrap();
    let mut b = SyntheticCorpus::new(shape, 0xC0FFEE).unwrap();
    for _ in 0..3 {
        let ba = a.next().await.unwrap();
        let bb = b.next().await.unwrap();
        assert_eq!(ba.zip_bytes, bb.zip_bytes, "same seed must produce same bytes");
        assert_eq!(ba.n_files, bb.n_files);
    }
}

#[tokio::test]
async fn bundles_stay_under_50mib_cap() {
    let shape = Shape::load_default().unwrap();
    let mut c = SyntheticCorpus::new(shape, 1).unwrap();
    for _ in 0..16 {
        let b = c.next().await.unwrap();
        assert!(
            b.zip_bytes.len() < 50 * 1024 * 1024,
            "bundle was {} bytes",
            b.zip_bytes.len()
        );
        assert!(b.n_files > 0);
    }
}

#[tokio::test]
async fn produced_bundle_is_a_valid_zip_with_log_entries() {
    let shape = Shape::load_default().unwrap();
    let mut c = SyntheticCorpus::new(shape, 7).unwrap();
    let b = c.next().await.unwrap();
    let cursor = std::io::Cursor::new(&b.zip_bytes);
    let mut z = zip::ZipArchive::new(cursor).expect("valid zip");
    assert!(!z.is_empty());
    let mut found_log = false;
    for i in 0..z.len() {
        let entry = z.by_index(i).unwrap();
        if entry.name().ends_with(".log") {
            found_log = true;
            break;
        }
    }
    assert!(found_log, "expected at least one .log file in the bundle");
}
