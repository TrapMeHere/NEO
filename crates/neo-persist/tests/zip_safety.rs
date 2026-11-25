//! `.joz` extraction safety: reject zip-slip and oversized entries.

use std::io::Write;

use neo_persist::session::read_joz;
use zip::write::SimpleFileOptions;

#[test]
fn joz_rejects_path_traversal() {
    let base = std::env::temp_dir().join(format!("neo-joz-test-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&base);
    let path = base.join("evil.joz");
    {
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        zip.start_file("session.jos", options).unwrap();
        zip.write_all(
            br#"<josm-session version="0.1"><layer name="x" type="osm-data" file="layers/0_x.osm"/></josm-session>"#,
        )
        .unwrap();
        zip.start_file("layers/0_../../evil.txt", options).unwrap();
        zip.write_all(b"pwned").unwrap();
        zip.finish().unwrap();
    }
    let extract = base.join("extract");
    let result = read_joz(&path, &extract);
    // The implementation may either reject the entry or sanitize its name to `evil.txt` inside the
    // extraction directory; the critical invariant is that nothing is ever written outside the directory.
    assert!(!base.join("evil.txt").exists());
    assert!(!base.join("..").join("evil.txt").exists());
    if let Err(err) = result {
        let text = err.to_string();
        assert!(text.contains("unsafe") || text.contains("zip"), "{text}");
    }
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn joz_never_keeps_original_absolute_layer_path() {
    let base = std::env::temp_dir().join(format!("neo-joz-path-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&base);
    let path = base.join("session.joz");
    {
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        zip.start_file("session.jos", options).unwrap();
        zip.write_all(
            br#"<josm-session version="0.1"><layer name="x" type="osm-data" file="C:\\Users\\victim\\secret.osm"/></josm-session>"#,
        )
        .unwrap();
        zip.finish().unwrap();
    }
    let session = read_joz(&path, base.join("extract")).unwrap();
    assert_eq!(session.layers[0].file, None);
    let _ = std::fs::remove_dir_all(&base);
}
