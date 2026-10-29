mod common;

use common::{command, command_with_api, json_stdout, ApiServer, TestDir};
use std::{fs, path::Path};

fn assert_selected_file_is_pushed(dir: &TestDir, args: &[&str], bytes: &[u8]) {
    let server = ApiServer::start();
    let output = command_with_api(args, &dir.config_dir(), server.endpoint())
        .env("SANDFLOS_TOKEN", "fixture-token")
        .output()
        .expect("run library push");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let value = json_stdout(&output);
    assert_eq!(value["data"]["accepted"], 1);
    assert_eq!(value["data"]["files"][0]["outcome"], "accepted");
    let uploads = server
        .requests()
        .into_iter()
        .filter(|request| request.method == "PUT" && request.path.starts_with("/uid-fixture/push/"))
        .collect::<Vec<_>>();
    assert_eq!(uploads.len(), 1);
    assert_eq!(uploads[0].body.as_bytes(), bytes);
}

fn configure(dir: &TestDir, kind: &str, flag: &str, path: &Path) {
    let output = command(
        &["--json", kind, "configure", flag, path.to_str().unwrap()],
        &dir.config_dir(),
    )
    .output()
    .expect("configure library");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(json_stdout(&output)["data"]["configured"], true);
}

#[test]
fn calibre_selected_format_uses_the_public_push_pipeline_without_modifying_the_database() {
    let dir = TestDir::new();
    let library = dir.path().join("Calibre Library");
    fs::create_dir_all(library.join("author/book")).unwrap();
    fs::write(library.join("author/book/selected.epub"), b"selected EPUB").unwrap();
    fs::write(library.join("author/book/other.pdf"), b"wrong format").unwrap();
    let db_path = library.join("metadata.db");
    let db = rusqlite::Connection::open(&db_path).unwrap();
    db.execute_batch("CREATE TABLE books(id INTEGER, title TEXT, author_sort TEXT, path TEXT, last_modified TEXT, pubdate TEXT);
        CREATE TABLE data(id INTEGER, book INTEGER, format TEXT, name TEXT, uncompressed_size INTEGER);
        INSERT INTO books VALUES(1,'Fixture','Author','author/book','2026-01-01','2000-01-01');
        INSERT INTO data VALUES(7,1,'EPUB','selected',13),(8,1,'PDF','other',12);").unwrap();
    drop(db);
    let original = fs::read(&db_path).unwrap();
    configure(&dir, "calibre", "--library", &library);
    assert_selected_file_is_pushed(
        &dir,
        &[
            "--json",
            "calibre",
            "push",
            "--library",
            library.to_str().unwrap(),
            "--data-id",
            "7",
        ],
        b"selected EPUB",
    );
    assert_eq!(fs::read(db_path).unwrap(), original);
}

#[test]
fn zotero_local_attachment_uses_the_public_push_pipeline_without_webdav() {
    let dir = TestDir::new();
    let library = dir.path().join("Zotero Data");
    fs::create_dir_all(library.join("storage/ABCDEFGH")).unwrap();
    fs::write(library.join("storage/ABCDEFGH/book.pdf"), b"selected PDF").unwrap();
    let db_path = library.join("zotero.sqlite");
    let db = rusqlite::Connection::open(&db_path).unwrap();
    db.execute_batch(
        "CREATE TABLE items(itemID INTEGER, key TEXT);
        CREATE TABLE itemAttachments(itemID INTEGER, path TEXT, linkMode INTEGER);
        INSERT INTO items VALUES(4,'ABCDEFGH');
        INSERT INTO itemAttachments VALUES(4,'storage:book.pdf',0);",
    )
    .unwrap();
    drop(db);
    let original = fs::read(&db_path).unwrap();
    configure(&dir, "zotero", "--data-dir", &library);
    assert_selected_file_is_pushed(
        &dir,
        &["--json", "zotero", "push", "--attachment-id", "4"],
        b"selected PDF",
    );
    assert_eq!(fs::read(db_path).unwrap(), original);
}
