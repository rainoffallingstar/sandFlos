use boox_core::cli_libraries::{calibre_directories, configure_calibre};
use std::fs;

fn canonical(path: impl AsRef<std::path::Path>) -> std::path::PathBuf {
    fs::canonicalize(path).expect("canonical path")
}

#[test]
fn library_configuration_is_isolated_and_validated() {
    let root = std::env::temp_dir().join(format!("sandflos-libraries-{}", uuid::Uuid::new_v4()));
    let first = root.join("first");
    let second = root.join("second");
    let library = root.join("books");
    fs::create_dir_all(&library).unwrap();
    let connection = rusqlite::Connection::open(library.join("metadata.db")).unwrap();
    connection
        .execute_batch("CREATE TABLE books(id INTEGER PRIMARY KEY);")
        .unwrap();
    configure_calibre(&first, vec![library.clone()]).unwrap();
    configure_calibre(&first, vec![library.clone()]).unwrap();
    assert_eq!(
        calibre_directories(&first).unwrap(),
        vec![canonical(&library)]
    );
    assert!(calibre_directories(&second).unwrap().is_empty());
    assert!(configure_calibre(&first, vec![root.join("missing")]).is_err());
    drop(connection);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn selected_library_and_format_resolve_the_expected_book() {
    use boox_core::cli_libraries::{calibre_file, calibre_list};
    let root = std::env::temp_dir().join(format!("sandflos-books-{}", uuid::Uuid::new_v4()));
    let library = root.join("books");
    let config = root.join("config");
    fs::create_dir_all(library.join("author/book")).unwrap();
    fs::write(library.join("author/book/sample.epub"), b"fixture book").unwrap();
    let db = rusqlite::Connection::open(library.join("metadata.db")).unwrap();
    db.execute_batch("CREATE TABLE books(id INTEGER, title TEXT, author_sort TEXT, path TEXT,last_modified TEXT,pubdate TEXT); CREATE TABLE data(id INTEGER,book INTEGER,format TEXT,name TEXT,uncompressed_size INTEGER); INSERT INTO books VALUES(1,'三体','刘慈欣','author/book','2026-01-01','2008-01-01'); INSERT INTO data VALUES(7,1,'EPUB','sample',12);").unwrap();
    configure_calibre(&config, vec![library.clone()]).unwrap();
    let books = calibre_list(&config, Some(10), Some(0), Some("三体".into())).unwrap();
    assert_eq!(books.len(), 1);
    assert_eq!(books[0].title, "三体");
    assert_eq!(
        calibre_file(&library, 7).unwrap(),
        canonical(library.join("author/book/sample.epub"))
    );
    assert!(calibre_file(&library, 8).is_err());
    drop(db);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn explicit_zotero_directory_resolves_local_attachment() {
    use boox_core::cli_libraries::*;
    let root = std::env::temp_dir().join(format!("sandflos-zotero-{}", uuid::Uuid::new_v4()));
    let data = root.join("data");
    let config = root.join("config");
    fs::create_dir_all(data.join("storage/ABCDEFGH")).unwrap();
    fs::write(data.join("storage/ABCDEFGH/book.pdf"), b"pdf").unwrap();
    let db = rusqlite::Connection::open(data.join("zotero.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE items(itemID INTEGER,key TEXT); CREATE TABLE itemAttachments(itemID INTEGER,path TEXT,linkMode INTEGER); INSERT INTO items VALUES(4,'ABCDEFGH'); INSERT INTO itemAttachments VALUES(4,'storage:book.pdf',0);").unwrap();
    configure_zotero(&config, &data).unwrap();
    assert_eq!(zotero_directory(&config).unwrap(), Some(canonical(&data)));
    let resolved = zotero_attachment(&config, 4, std::time::Duration::from_secs(2)).unwrap();
    assert_eq!(fs::read(&resolved.path).unwrap(), b"pdf");
    drop(resolved);
    drop(db);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn zotero_list_reads_explicit_database_and_filters_titles() {
    use boox_core::cli_libraries::*;
    let temp_dir = std::env::temp_dir().join(format!("cli-zotero-list-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_dir).unwrap();
    let conn = rusqlite::Connection::open(temp_dir.join("zotero.sqlite")).unwrap();
    conn.execute_batch(
            "
            CREATE TABLE libraries (libraryID INTEGER PRIMARY KEY, type TEXT, editable INT, filesEditable INT, version INT, storageVersion INT, lastSync INT, archived INT);
            CREATE TABLE itemTypes (itemTypeID INTEGER PRIMARY KEY, typeName TEXT);
            CREATE TABLE items (itemID INTEGER PRIMARY KEY, itemTypeID INT, dateAdded TEXT, dateModified TEXT, clientDateModified TEXT, libraryID INT, key TEXT, version INT, synced INT);
            CREATE TABLE fields (fieldID INTEGER PRIMARY KEY, fieldName TEXT, fieldFormatID INT);
            CREATE TABLE itemData (itemID INT, fieldID INT, valueID INT);
            CREATE TABLE itemDataValues (valueID INTEGER PRIMARY KEY, value TEXT);
            CREATE TABLE creators (creatorID INTEGER PRIMARY KEY, firstName TEXT, lastName TEXT, fieldMode INT);
            CREATE TABLE itemCreators (itemID INT, creatorID INT, creatorTypeID INT, orderIndex INT);
            CREATE TABLE itemAttachments (itemID INTEGER PRIMARY KEY, parentItemID INT, linkMode INT, contentType TEXT, charsetID INT, path TEXT, syncState INT, storageModTime INT, storageHash TEXT, lastProcessedModificationTime INT);
            ",
        )
        .unwrap();
    conn.execute("INSERT INTO libraries (libraryID, type, editable, filesEditable, version, storageVersion, lastSync, archived) VALUES (1, 'user', 1, 1, 0, 0, 0, 0)", []).unwrap();
    conn.execute(
        "INSERT INTO itemTypes (itemTypeID, typeName) VALUES (3, 'attachment'), (7, 'book')",
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO items (itemID, itemTypeID, dateAdded, dateModified, clientDateModified, libraryID, key, version, synced) VALUES (1, 7, '2024-01-01', '2024-01-02', '2024-01-02', 1, 'BOOK1', 0, 0), (2, 3, '2024-01-01', '2024-01-02', '2024-01-02', 1, 'ATT1', 0, 0)", []).unwrap();
    conn.execute("INSERT INTO fields (fieldID, fieldName, fieldFormatID) VALUES (1, 'title', 0), (2, 'date', 0)", []).unwrap();
    conn.execute(
        "INSERT INTO itemDataValues (valueID, value) VALUES (1, 'Test Book'), (2, '2024-03-01')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO itemData (itemID, fieldID, valueID) VALUES (1, 1, 1), (1, 2, 2)",
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO creators (creatorID, firstName, lastName, fieldMode) VALUES (1, 'Ada', 'Lovelace', 0)", []).unwrap();
    conn.execute("INSERT INTO itemCreators (itemID, creatorID, creatorTypeID, orderIndex) VALUES (1, 1, 1, 0)", []).unwrap();
    conn.execute("INSERT INTO itemAttachments (itemID, parentItemID, linkMode, contentType, charsetID, path, syncState, storageModTime, storageHash, lastProcessedModificationTime) VALUES (2, 1, 0, 'application/pdf', 0, 'storage:test.pdf', 0, 0, '', 0)", []).unwrap();
    drop(conn);

    let config = temp_dir.join("config");
    configure_zotero(&config, &temp_dir).unwrap();
    let items = zotero_list(&config, Some(10), Some(0), Some("Test".into())).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].title, "Test Book");
    assert_eq!(items[0].attachments.len(), 1);
    assert!(
        zotero_list(&config, Some(10), Some(0), Some("missing".into()))
            .unwrap()
            .is_empty()
    );
    fs::remove_dir_all(temp_dir).unwrap();
}
#[test]
fn missing_local_attachment_downloads_webdav_and_cleans_up() {
    webdav_archive_case("book.pdf", true, 200);
}
#[test]
fn webdav_rejects_archive_path_traversal() {
    webdav_archive_case("../escape.pdf", false, 200);
}
#[test]
fn webdav_rejects_archive_without_requested_attachment() {
    webdav_archive_case("unrelated.pdf", false, 200);
}
#[test]
fn webdav_reports_authentication_failures() {
    webdav_archive_case("book.pdf", false, 401);
}
#[test]
fn webdav_password_can_contain_spaces() {
    webdav_archive_case_with_password("book.pdf", true, 200, "correct horse battery staple");
}
fn webdav_archive_case(entry_name: &'static str, accepted: bool, status: u16) {
    webdav_archive_case_with_password(entry_name, accepted, status, "fake-password");
}
fn webdav_archive_case_with_password(
    entry_name: &'static str,
    accepted: bool,
    status: u16,
    password: &'static str,
) {
    use boox_core::{auth_service::CredentialStoreKind, cli_libraries::*};
    use std::io::{Read, Write};
    let root = std::env::temp_dir().join(format!("cli-webdav-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let db = rusqlite::Connection::open(root.join("zotero.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE items(itemID INTEGER,key TEXT); CREATE TABLE itemAttachments(itemID INTEGER,path TEXT,linkMode INTEGER); INSERT INTO items VALUES(4,'ABCDEFGH'); INSERT INTO itemAttachments VALUES(4,'storage:book.pdf',0);").unwrap();
    let config = root.join("config");
    configure_zotero(&config, &root).unwrap();
    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/zotero", server.local_addr().unwrap());
    configure_webdav_with_store(&config, &url, "user", password, CredentialStoreKind::File)
        .unwrap();
    let worker = std::thread::spawn(move || {
        server.set_nonblocking(true).unwrap();
        let start = std::time::Instant::now();
        let mut stream = loop {
            match server.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && start.elapsed() < std::time::Duration::from_secs(5) =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(5))
                }
                Err(_) => return,
            }
        };
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        let mut request = [0u8; 4096];
        let len = stream.read(&mut request).unwrap();
        assert!(String::from_utf8_lossy(&request[..len]).starts_with("GET /zotero/ABCDEFGH.zip "));
        if password == "correct horse battery staple" {
            assert!(String::from_utf8_lossy(&request[..len])
                .contains("Basic dXNlcjpjb3JyZWN0IGhvcnNlIGJhdHRlcnkgc3RhcGxl"));
        }
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file(entry_name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"remote pdf").unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        write!(
            stream,
            "HTTP/1.1 {status} Response\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len()
        )
        .unwrap();
        stream.write_all(&bytes).unwrap();
    });
    let result = zotero_attachment(&config, 4, std::time::Duration::from_secs(3));
    if accepted {
        let resolved = result.unwrap();
        assert_eq!(fs::read(&resolved.path).unwrap(), b"remote pdf");
        let path = resolved.path.clone();
        drop(resolved);
        assert!(!path.exists());
    } else {
        let error = result.err().expect("archive should fail");
        if status == 401 {
            assert_eq!(
                error.kind,
                boox_core::services::ServiceErrorKind::Authentication
            );
        }
    }
    worker.join().unwrap();
    drop(db);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn configuration_rejects_an_unrelated_sqlite_database() {
    use boox_core::cli_libraries::*;
    let root = std::env::temp_dir().join(format!("cli-wrong-db-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    drop(rusqlite::Connection::open(root.join("metadata.db")).unwrap());
    drop(rusqlite::Connection::open(root.join("zotero.sqlite")).unwrap());
    assert!(configure_calibre(&root.join("config"), vec![root.clone()]).is_err());
    assert!(configure_zotero(&root.join("config"), &root).is_err());
    fs::remove_dir_all(root).unwrap();
}
