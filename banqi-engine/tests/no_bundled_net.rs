mod common;
use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
};

fn net_files(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "target") {
                continue;
            }
            found.extend(net_files(&path)?);
        } else if path.is_file() {
            let mut magic = Vec::with_capacity(4);
            File::open(&path)?.take(4).read_to_end(&mut magic)?;
            if path.as_os_str().as_encoded_bytes().ends_with(b".bqnn") || magic == b"BQNN" {
                found.push(path);
            }
        }
    }
    Ok(found)
}

#[test]
fn repository_contains_no_net_files_or_disguised_bqnn_payloads() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let found = net_files(root).expect("scan repository files");
    assert!(
        found.is_empty(),
        "net files must stay outside the repository: {found:?}"
    );
}

#[test]
fn scanner_catches_extension_and_magic_and_excludes_target() {
    let fake = common::TestFile::new(b"fake net, no magic");
    let root = fake.path.parent().unwrap();
    assert_eq!(net_files(root).unwrap(), [fake.path.clone()]);

    let hidden = root.join(".bqnn");
    fs::rename(&fake.path, &hidden).unwrap();
    assert_eq!(net_files(root).unwrap(), [hidden.clone()]);

    let disguised = root.join("weights.dat");
    fs::rename(&hidden, &disguised).unwrap();
    fs::write(&disguised, b"BQNN fake header").unwrap();
    assert_eq!(net_files(root).unwrap(), [disguised.clone()]);

    let target = root.join("target");
    fs::create_dir(&target).unwrap();
    fs::rename(&disguised, target.join("fake.bqnn")).unwrap();
    assert!(net_files(root).unwrap().is_empty());
    fs::rename(target.join("fake.bqnn"), &fake.path).unwrap();
    fs::remove_dir(target).unwrap();
}
