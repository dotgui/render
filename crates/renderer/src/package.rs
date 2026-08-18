use std::{
    collections::BTreeMap,
    io::{Cursor, Read},
};
use thiserror::Error;
use zip::ZipArchive;

#[derive(Debug, Error)]
pub enum PackageError {
    #[error("invalid .gui package: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("failed to read design.guix: {0}")]
    Read(#[from] std::io::Error),

    #[error(".gui package is missing design.guix")]
    MissingDesign,

    #[error("design.guix is not valid UTF-8")]
    Utf8(#[from] std::string::FromUtf8Error),
}

#[derive(Debug, Clone)]
pub struct GuiPackage {
    pub xml: String,
    pub assets: BTreeMap<String, Vec<u8>>,
}

pub fn read_gui_package_xml(bytes: &[u8]) -> Result<String, PackageError> {
    read_gui_package(bytes).map(|package| package.xml)
}

pub fn read_gui_package(bytes: &[u8]) -> Result<GuiPackage, PackageError> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    let mut xml = None;
    let mut assets = BTreeMap::new();

    for index in 0..archive.len() {
        let mut file = archive.by_index(index)?;
        if file.is_dir() {
            continue;
        }

        let name = file.name().to_owned();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;

        if name == "design.guix" {
            xml = Some(String::from_utf8(bytes)?);
        } else {
            assets.insert(name, bytes);
        }
    }

    Ok(GuiPackage {
        xml: xml.ok_or(PackageError::MissingDesign)?,
        assets,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::{write::SimpleFileOptions, ZipWriter};

    #[test]
    fn reads_design_and_packaged_assets() {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut bytes);
            zip.start_file("design.guix", SimpleFileOptions::default())
                .expect("design file starts");
            zip.write_all(b"<gui version=\"0.2\"><col /></gui>")
                .expect("design writes");
            zip.start_file("assets/icon.svg", SimpleFileOptions::default())
                .expect("asset file starts");
            zip.write_all(b"<svg />").expect("asset writes");
            zip.finish().expect("zip finishes");
        }

        let package = read_gui_package(&bytes.into_inner()).expect("package reads");

        assert!(package.xml.contains("<gui"));
        assert_eq!(
            package.assets.get("assets/icon.svg").map(Vec::as_slice),
            Some(b"<svg />".as_slice())
        );
    }
}
