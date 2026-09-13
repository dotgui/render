//! Helpers shared by the integration tests.

use std::{
    fs,
    io::{Cursor, Write as _},
    path::Path,
};

/// A package directory as the `.gui` a producer would write.
pub fn zip_dir(dir: &Path) -> Vec<u8> {
    fn walk(dir: &Path, root: &Path, files: &mut Vec<(String, Vec<u8>)>) {
        for entry in fs::read_dir(dir).expect("package directory is readable") {
            let path = entry.expect("package entry is readable").path();
            if path.is_dir() {
                walk(&path, root, files);
            } else {
                let name = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                files.push((name, fs::read(&path).expect("package file is readable")));
            }
        }
    }

    let mut files = Vec::new();
    walk(dir, dir, &mut files);
    files.sort();

    let mut bytes = Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(&mut bytes);
    for (name, contents) in files {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .expect("zip entry starts");
        zip.write_all(&contents).expect("zip entry writes");
    }
    zip.finish().expect("zip finishes");
    bytes.into_inner()
}
