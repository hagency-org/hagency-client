use super::*;

#[test]
fn graph_history_capture_never_creates_missing_documents_and_detects_new_files() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().canonicalize().unwrap().join("source");
    private::create_directory_new(&root).unwrap();
    let capture = GraphDocuments::capture(&root).unwrap();
    assert_eq!(capture.summary().present_documents, 0);
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    private::write_new(&root.join("messages.json"), b"[]").unwrap();
    assert!(capture.check().is_err());
    assert_eq!(std::fs::read(root.join("messages.json")).unwrap(), b"[]");
}

#[test]
fn graph_history_checks_same_file_bytes_removal_and_unchanged_root() {
    for action in ["rewrite", "replace", "remove", "root"] {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().canonicalize().unwrap().join("source");
        private::create_directory_new(&root).unwrap();
        let path = root.join("messages.json");
        private::write_new(&path, b"[]").unwrap();
        let capture = GraphDocuments::capture(&root).unwrap();
        capture.check().unwrap();
        match action {
            "rewrite" => std::fs::write(&path, b"[ ]").unwrap(),
            "replace" => private::replace(&path, b"[]").unwrap(),
            "remove" => std::fs::remove_file(&path).unwrap(),
            "root" => {
                std::fs::rename(&root, root.with_file_name("old")).unwrap();
                private::create_directory_new(&root).unwrap();
                private::write_new(&path, b"[]").unwrap();
            }
            _ => unreachable!(),
        }
        assert!(capture.check().is_err(), "{action}");
    }
}

#[cfg(unix)]
#[test]
fn graph_history_refuses_symlinks_public_permissions_and_ambiguous_json_without_repair() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().canonicalize().unwrap().join("source");
    private::create_directory_new(&root).unwrap();
    let path = root.join("messages.json");
    private::write_new(&path, b"{}").unwrap();
    assert!(GraphDocuments::capture(&root).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"{}");
    private::replace(&path, b"[]").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(GraphDocuments::capture(&root).is_err());
    std::fs::remove_file(&path).unwrap();
    let outside = root.with_file_name("outside");
    private::write_new(&outside, b"[]").unwrap();
    symlink(&outside, &path).unwrap();
    assert!(GraphDocuments::capture(&root).is_err());
    assert_eq!(std::fs::read(&outside).unwrap(), b"[]");
    std::fs::remove_file(&path).unwrap();
    let alias = root.with_file_name("alias");
    symlink(&root, &alias).unwrap();
    assert!(GraphDocuments::capture(&alias).is_err());
}
