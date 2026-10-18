use std::fs;

#[test]
fn replacement_failure_keeps_existing_directory_and_cleans_temp() {
    let dir = tempfile_dir("failure");
    let path = dir.join("target");
    fs::create_dir(&path).unwrap();
    fs::write(path.join("sentinel"), "old").unwrap();

    assert!(super::write_atomic(&path, b"new").is_err());
    assert_eq!(fs::read_to_string(path.join("sentinel")).unwrap(), "old");
    assert!(
        fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .all(|entry| !entry.file_name().to_string_lossy().contains(".tmp-"))
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn concurrent_replacements_use_unique_same_directory_temporaries() {
    let dir = tempfile_dir("concurrent");
    let path = std::sync::Arc::new(dir.join("config.toml"));
    let mut threads = Vec::new();
    for i in 0..8 {
        let path = path.clone();
        threads.push(std::thread::spawn(move || {
            super::write_atomic(&path, format!("value-{i}").as_bytes()).unwrap();
        }));
    }
    for thread in threads {
        thread.join().unwrap();
    }
    let value = fs::read_to_string(&*path).unwrap();
    assert!(value.starts_with("value-"));
    assert!(
        fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .all(|entry| !entry.file_name().to_string_lossy().contains(".tmp-"))
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn atomic_temp_names_are_recognized_without_matching_hidden_files() {
    let current_pid = std::process::id();
    assert!(super::is_atomic_temp_path(std::path::Path::new(&format!(
        ".cover.tmp-{current_pid}-123-0"
    ))));
    assert!(!super::is_atomic_temp_path(std::path::Path::new(
        ".cover.tmp"
    )));
    assert!(!super::is_atomic_temp_path(std::path::Path::new(
        "cover.tmp-1-2-3"
    )));
}

#[cfg(unix)]
#[test]
fn explicit_cookie_mode_is_private_and_general_mode_is_preserved() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile_dir("permissions");
    let path = dir.join("session.toml");
    super::write_atomic_with_mode(&path, b"cookie", Some(0o600)).unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    super::write_atomic(&path, b"updated").unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
    let _ = fs::remove_dir_all(dir);
}

fn tempfile_dir(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "cnmplayer-atomic-test-{}-{name}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}
