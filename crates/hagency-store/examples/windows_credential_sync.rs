#[cfg(windows)]
fn main() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("private-original");
    hagency_store::private::directory(&root).unwrap();
    let retained = cap_std::fs::Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
    let mut sync = hagency_store::private::WindowsDirectorySync::open(&retained)
        .expect("original directory inspection")
        .expect("original directory sync capability");
    sync.sync().expect("actual directory flush");
    println!("original directory flush passed");
    let original = hagency_store::credential_vault::document::installation(&root, true)
        .expect("actual original installation publication");
    assert_eq!(
        hagency_store::credential_vault::document::installation(&root, false).unwrap(),
        original
    );
    println!("original installation durable reopen passed");
}
#[cfg(not(windows))]
fn main() {}
