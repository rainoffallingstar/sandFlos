use boox_core::cli_libraries::{calibre_directories, configure_calibre};
use std::fs;

fn canonical(path: impl AsRef<std::path::Path>) -> std::path::PathBuf {
    fs::canonicalize(path).expect("canonical path")
}

/// A missing client must not leave an accept blocked after a failing test.
fn accept_bounded(listener: std::net::TcpListener) -> Option<std::net::TcpStream> {
    listener.set_nonblocking(true).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(std::time::Duration::from_secs(3)))
                    .unwrap();
                return Some(stream);
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(_) => return None,
        }
    }
}

/// Join a fixture thread without allowing a wedged test helper to hang the test.
fn join_bounded(worker: std::thread::JoinHandle<()>) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while !worker.is_finished() {
        assert!(
            std::time::Instant::now() < deadline,
            "fixture thread did not finish before its deadline"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    worker.join().expect("fixture thread panicked");
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
        .execute_batch(
            "CREATE TABLE books(id INTEGER PRIMARY KEY,title TEXT,author_sort TEXT,path TEXT,last_modified TEXT,pubdate TEXT);
             CREATE TABLE data(id INTEGER,book INTEGER,format TEXT,name TEXT,uncompressed_size INTEGER);",
        )
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
fn calibre_configuration_rejects_a_books_table_without_the_required_schema() {
    let root = std::env::temp_dir().join(format!("calibre-schema-{}", uuid::Uuid::new_v4()));
    let library = root.join("library");
    let config = root.join("config");
    fs::create_dir_all(&library).unwrap();
    let database = library.join("metadata.db");
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch("CREATE TABLE books(id INTEGER PRIMARY KEY);")
        .unwrap();
    drop(connection);
    let before = fs::read(&database).unwrap();
    let result = configure_calibre(&config, vec![library]);
    let after = fs::read(&database).unwrap();
    let configured = calibre_directories(&config).unwrap();
    fs::remove_dir_all(root).unwrap();
    let error = result.expect_err("a coincidental books.id column is not a Calibre schema");
    assert_eq!(error.kind, boox_core::services::ServiceErrorKind::LocalIo);
    assert!(configured.is_empty());
    assert_eq!(before, after);
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
    use std::io::{BufRead, BufReader, Write};
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
        let Some(mut stream) = accept_bounded(server) else {
            return;
        };
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request = String::new();
        loop {
            let mut line = String::new();
            assert!(
                reader.read_line(&mut line).unwrap() > 0,
                "incomplete request"
            );
            let complete = line == "\r\n";
            request.push_str(&line);
            if complete {
                break;
            }
        }
        assert!(request.starts_with("GET /zotero/ABCDEFGH.zip "));
        if password == "correct horse battery staple" {
            assert!(request.contains("Basic dXNlcjpjb3JyZWN0IGhvcnNlIGJhdHRlcnkgc3RhcGxl"));
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
    join_bounded(worker);
    drop(db);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn webdav_corrupt_archive_is_a_typed_error() {
    use boox_core::{auth_service::CredentialStoreKind, cli_libraries::*};
    use std::io::{BufRead, Write};

    let root = std::env::temp_dir().join(format!("cli-webdav-corrupt-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let db = rusqlite::Connection::open(root.join("zotero.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE items(itemID INTEGER,key TEXT); CREATE TABLE itemAttachments(itemID INTEGER,path TEXT,linkMode INTEGER); INSERT INTO items VALUES(4,'ABCDEFGH'); INSERT INTO itemAttachments VALUES(4,'storage:book.pdf',0);").unwrap();
    let config = root.join("config");
    configure_zotero(&config, &root).unwrap();
    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/zotero", server.local_addr().unwrap());
    configure_webdav_with_store(&config, &url, "user", "password", CredentialStoreKind::File)
        .unwrap();
    let worker = std::thread::spawn(move || {
        let Some(mut stream) = accept_bounded(server) else {
            return;
        };
        let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap() > 0 {
            if line == "\r\n" {
                break;
            }
            line.clear();
        }
        let body = b"this is not a zip archive";
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(body).unwrap();
    });
    let error = zotero_attachment(&config, 4, std::time::Duration::from_secs(3))
        .err()
        .expect("corrupt archive must be rejected");
    join_bounded(worker);
    assert_eq!(error.kind, boox_core::services::ServiceErrorKind::LocalIo);
    assert_eq!(error.message, "invalid WebDAV ZIP");
    drop(db);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn webdav_missing_archive_is_not_found() {
    use boox_core::{auth_service::CredentialStoreKind, cli_libraries::*};
    use std::io::{BufRead, Write};

    let root = std::env::temp_dir().join(format!("cli-webdav-404-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let db = rusqlite::Connection::open(root.join("zotero.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE items(itemID INTEGER,key TEXT); CREATE TABLE itemAttachments(itemID INTEGER,path TEXT,linkMode INTEGER); INSERT INTO items VALUES(4,'ABCDEFGH'); INSERT INTO itemAttachments VALUES(4,'storage:book.pdf',0);").unwrap();
    let config = root.join("config");
    configure_zotero(&config, &root).unwrap();
    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/zotero", server.local_addr().unwrap());
    configure_webdav_with_store(&config, &url, "user", "password", CredentialStoreKind::File)
        .unwrap();
    let worker = std::thread::spawn(move || {
        let Some(mut stream) = accept_bounded(server) else {
            return;
        };
        let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap() > 0 {
            if line == "\r\n" {
                break;
            }
            line.clear();
        }
        write!(
            stream,
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
    });
    let error = zotero_attachment(&config, 4, std::time::Duration::from_secs(3))
        .err()
        .expect("missing archive must fail");
    join_bounded(worker);
    assert_eq!(error.kind, boox_core::services::ServiceErrorKind::NotFound);
    assert_eq!(error.message, "WebDAV attachment not found");
    drop(db);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn webdav_expanded_archive_over_limit_is_rejected_before_extraction() {
    use boox_core::{auth_service::CredentialStoreKind, cli_libraries::*};
    use std::io::{BufRead, Write};

    let root = std::env::temp_dir().join(format!("cli-webdav-large-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let db = rusqlite::Connection::open(root.join("zotero.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE items(itemID INTEGER,key TEXT); CREATE TABLE itemAttachments(itemID INTEGER,path TEXT,linkMode INTEGER); INSERT INTO items VALUES(4,'ABCDEFGH'); INSERT INTO itemAttachments VALUES(4,'storage:book.pdf',0);").unwrap();
    let config = root.join("config");
    configure_zotero(&config, &root).unwrap();
    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/zotero", server.local_addr().unwrap());
    configure_webdav_with_store(&config, &url, "user", "password", CredentialStoreKind::File)
        .unwrap();
    let worker = std::thread::spawn(move || {
        let Some(mut stream) = accept_bounded(server) else {
            return;
        };
        let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap() > 0 {
            if line == "\r\n" {
                break;
            }
            line.clear();
        }

        // Build a structurally valid ZIP whose payload expands past the
        // 200 MiB extraction limit while the compressed archive stays small.
        let payload_len = 200_usize * 1024 * 1024 + 1;
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file(
            "book.pdf",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
        let chunk = vec![0u8; 1024 * 1024];
        let mut remaining = payload_len;
        while remaining > 0 {
            let take = remaining.min(chunk.len());
            zip.write_all(&chunk[..take]).unwrap();
            remaining -= take;
        }
        let bytes = zip.finish().unwrap().into_inner();
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len()
        );
        // The client can reject the expanded size before the body finishes,
        // so a reset while the fixture is still writing is expected.
        let _ = stream.write_all(header.as_bytes());
        let _ = stream.write_all(&bytes);
    });
    let error = zotero_attachment(&config, 4, std::time::Duration::from_secs(30))
        .err()
        .expect("oversized expanded archive must fail");
    join_bounded(worker);
    assert_eq!(error.kind, boox_core::services::ServiceErrorKind::LocalIo);
    assert_eq!(error.message, "expanded ZIP too large");
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

    let corrupt = root.join("corrupt");
    fs::create_dir_all(&corrupt).unwrap();
    fs::write(corrupt.join("metadata.db"), b"not a sqlite database").unwrap();
    let before = fs::read(corrupt.join("metadata.db")).unwrap();
    assert!(configure_calibre(&root.join("config"), vec![corrupt.clone()]).is_err());
    assert_eq!(fs::read(corrupt.join("metadata.db")).unwrap(), before);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn calibre_lists_duplicate_ids_from_separate_libraries_with_their_own_paths() {
    use boox_core::cli_libraries::*;

    let root = std::env::temp_dir().join(format!("cli-two-libs-{}", uuid::Uuid::new_v4()));
    let first = root.join("first");
    let second = root.join("second");
    for (library, title, file) in [
        (&first, "First Book", "first.epub"),
        (&second, "Second Book", "second.epub"),
    ] {
        fs::create_dir_all(library.join("author/book")).unwrap();
        fs::write(library.join("author/book").join(file), title.as_bytes()).unwrap();
        let db = rusqlite::Connection::open(library.join("metadata.db")).unwrap();
        db.execute_batch(&format!(
            "CREATE TABLE books(id INTEGER, title TEXT, author_sort TEXT, path TEXT, last_modified TEXT, pubdate TEXT);
             CREATE TABLE data(id INTEGER, book INTEGER, format TEXT, name TEXT, uncompressed_size INTEGER);
             INSERT INTO books VALUES(1,'{title}','Author','author/book','2026-01-01','2000-01-01');
             INSERT INTO data VALUES(7,1,'EPUB','{}','{}');",
            file.trim_end_matches(".epub"),
            title.len()
        ))
        .unwrap();
        drop(db);
    }

    let config = root.join("config");
    configure_calibre(&config, vec![first.clone(), second.clone()]).unwrap();
    let books = calibre_list(&config, None, None, None).unwrap();
    assert_eq!(books.len(), 2, "{books:#?}");
    let mut resolved = books
        .iter()
        .map(|book| {
            let format = book
                .formats
                .iter()
                .find(|format| format.data_id == 7)
                .expect("format id 7");
            (
                book.library_dir.clone(),
                fs::read(calibre_file(std::path::Path::new(&format.library_dir), 7).unwrap())
                    .unwrap(),
            )
        })
        .collect::<Vec<_>>();
    resolved.sort();
    assert_eq!(
        resolved,
        vec![
            (first.to_string_lossy().to_string(), b"First Book".to_vec()),
            (
                second.to_string_lossy().to_string(),
                b"Second Book".to_vec()
            ),
        ]
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn webdav_rejects_archive_symlink_entries() {
    use boox_core::{auth_service::CredentialStoreKind, cli_libraries::*};
    use std::io::{BufRead, Write};

    let root = std::env::temp_dir().join(format!("cli-webdav-symlink-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let db = rusqlite::Connection::open(root.join("zotero.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE items(itemID INTEGER,key TEXT); CREATE TABLE itemAttachments(itemID INTEGER,path TEXT,linkMode INTEGER); INSERT INTO items VALUES(4,'ABCDEFGH'); INSERT INTO itemAttachments VALUES(4,'storage:book.pdf',0);").unwrap();
    let config = root.join("config");
    configure_zotero(&config, &root).unwrap();
    let server = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let url = format!("http://{}/zotero", server.local_addr().unwrap());
    configure_webdav_with_store(&config, &url, "user", "password", CredentialStoreKind::File)
        .unwrap();
    let worker = std::thread::spawn(move || {
        let Some(mut stream) = accept_bounded(server) else {
            return;
        };
        let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap() > 0 {
            if line == "\r\n" {
                break;
            }
            line.clear();
        }
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.add_symlink(
            "book.pdf",
            "../../outside.pdf",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len()
        )
        .unwrap();
        stream.write_all(&bytes).unwrap();
    });
    let error = zotero_attachment(&config, 4, std::time::Duration::from_secs(3))
        .err()
        .expect("symlink archive must fail");
    join_bounded(worker);
    assert_eq!(error.kind, boox_core::services::ServiceErrorKind::LocalIo);
    assert_eq!(error.message, "unsafe ZIP path");
    drop(db);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unreadable_webdav_credentials_fail_before_opening_a_connection() {
    use boox_core::{auth_service::CredentialStoreKind, cli_libraries::*};
    use std::io::ErrorKind;

    for corrupt in [false, true] {
        let root = std::env::temp_dir().join(format!(
            "cli-webdav-bad-credential-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        let db = rusqlite::Connection::open(root.join("zotero.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE items(itemID INTEGER,key TEXT); CREATE TABLE itemAttachments(itemID INTEGER,path TEXT,linkMode INTEGER); INSERT INTO items VALUES(4,'ABCDEFGH'); INSERT INTO itemAttachments VALUES(4,'storage:book.pdf',0);").unwrap();
        let config = root.join("config");
        configure_zotero(&config, &root).unwrap();
        let server = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        server.set_nonblocking(true).unwrap();
        let url = format!("http://{}/zotero", server.local_addr().unwrap());
        configure_webdav_with_store(&config, &url, "user", "password", CredentialStoreKind::File)
            .unwrap();

        let credential = config.join("webdav").join("auth.json");
        if corrupt {
            fs::write(&credential, b"not valid credential json").unwrap();
        } else {
            fs::remove_file(&credential).unwrap();
            fs::create_dir(&credential).unwrap();
            fs::write(
                credential.join("unexpected"),
                b"directory in place of a file",
            )
            .unwrap();
        }

        let error = zotero_attachment(&config, 4, std::time::Duration::from_secs(2))
            .err()
            .expect("unreadable WebDAV credentials must fail");
        assert_eq!(error.kind, boox_core::services::ServiceErrorKind::LocalIo);
        assert_eq!(error.message, "WebDAV credentials unavailable");
        assert!(
            matches!(server.accept(), Err(error) if error.kind() == ErrorKind::WouldBlock),
            "credential failure must happen before any WebDAV request"
        );
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn calibre_corruption_after_configuration_is_reported_without_modifying_the_source() {
    use boox_core::cli_libraries::*;

    let root = std::env::temp_dir().join(format!("cli-calibre-corrupt-{}", uuid::Uuid::new_v4()));
    let library = root.join("books");
    fs::create_dir_all(library.join("author/book")).unwrap();
    fs::write(library.join("author/book/book.epub"), b"book").unwrap();
    let db_path = library.join("metadata.db");
    let db = rusqlite::Connection::open(&db_path).unwrap();
    db.execute_batch(
        "CREATE TABLE books(id INTEGER, title TEXT, author_sort TEXT, path TEXT, last_modified TEXT, pubdate TEXT);
         CREATE TABLE data(id INTEGER, book INTEGER, format TEXT, name TEXT, uncompressed_size INTEGER);
         INSERT INTO books VALUES(1,'Book','Author','author/book','2026-01-01','2000-01-01');
         INSERT INTO data VALUES(7,1,'EPUB','book',4);",
    )
    .unwrap();
    drop(db);
    let config = root.join("config");
    configure_calibre(&config, vec![library]).unwrap();

    fs::write(&db_path, b"not a sqlite database").unwrap();
    let corrupted = fs::read(&db_path).unwrap();
    let error = calibre_list(&config, None, None, None).unwrap_err();
    assert_eq!(error.kind, boox_core::services::ServiceErrorKind::LocalIo);
    assert_eq!(error.message, "cannot query Calibre database");
    assert_eq!(fs::read(&db_path).unwrap(), corrupted);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn calibre_listing_is_paged_globally_across_libraries() {
    use boox_core::cli_libraries::*;

    let root = std::env::temp_dir().join(format!("cli-calibre-page-{}", uuid::Uuid::new_v4()));
    let first = root.join("first");
    let second = root.join("second");
    for (library, books) in [
        (&first, [("Alpha", "2026-01-01"), ("Gamma", "2026-01-03")]),
        (&second, [("Beta", "2026-01-02"), ("Delta", "2026-01-04")]),
    ] {
        for (index, (title, _date)) in books.into_iter().enumerate() {
            let book_dir = format!("author/book{}", index + 1);
            fs::create_dir_all(library.join(&book_dir)).unwrap();
            fs::write(library.join(&book_dir).join("book.epub"), title.as_bytes()).unwrap();
        }
        let db = rusqlite::Connection::open(library.join("metadata.db")).unwrap();
        db.execute_batch(
            "CREATE TABLE books(id INTEGER, title TEXT, author_sort TEXT, path TEXT, last_modified TEXT, pubdate TEXT);
             CREATE TABLE data(id INTEGER, book INTEGER, format TEXT, name TEXT, uncompressed_size INTEGER);",
        )
        .unwrap();
        for (index, (title, date)) in books.into_iter().enumerate() {
            let book_id = index as i64 + 1;
            db.execute(
                "INSERT INTO books VALUES(?1,?2,'Author',?3,?4,'2000-01-01')",
                rusqlite::params![book_id, title, format!("author/book{}", index + 1), date],
            )
            .unwrap();
            db.execute(
                "INSERT INTO data VALUES(?1,?2,'EPUB','book',4)",
                rusqlite::params![book_id, book_id],
            )
            .unwrap();
        }
        drop(db);
    }

    let config = root.join("config");
    let first_db = first.join("metadata.db");
    let second_db = second.join("metadata.db");
    let first_bytes = fs::read(&first_db).unwrap();
    let second_bytes = fs::read(&second_db).unwrap();
    configure_calibre(&config, vec![first.clone(), second.clone()]).unwrap();
    let titles = |limit, offset, search: Option<&str>| {
        calibre_list(&config, limit, offset, search.map(str::to_string))
            .unwrap()
            .into_iter()
            .map(|book| book.title)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        titles(None, None, None),
        ["Delta", "Gamma", "Beta", "Alpha"]
    );
    assert_eq!(titles(Some(2), Some(1), None), ["Gamma", "Beta"]);
    assert!(titles(Some(2), Some(10), None).is_empty());
    assert_eq!(
        titles(None, None, Some("a")),
        ["Delta", "Gamma", "Beta", "Alpha"]
    );
    assert_eq!(titles(Some(1), Some(1), Some("a")), ["Gamma"]);
    assert_eq!(titles(None, None, Some("alpha")), ["Alpha"]);
    assert_eq!(titles(None, None, Some("author")).len(), 4);
    assert!(titles(None, None, Some("missing")).is_empty());
    assert_eq!(fs::read(&first_db).unwrap(), first_bytes);
    assert_eq!(fs::read(&second_db).unwrap(), second_bytes);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn calibre_listing_reports_a_locked_database_without_hanging_and_can_be_retried() {
    use boox_core::cli_libraries::*;

    let root = std::env::temp_dir().join(format!("cli-calibre-lock-{}", uuid::Uuid::new_v4()));
    let library = root.join("books");
    fs::create_dir_all(library.join("author/book")).unwrap();
    fs::write(library.join("author/book/book.epub"), b"book").unwrap();
    let db_path = library.join("metadata.db");
    let db = rusqlite::Connection::open(&db_path).unwrap();
    db.execute_batch(
        "CREATE TABLE books(id INTEGER, title TEXT, author_sort TEXT, path TEXT, last_modified TEXT, pubdate TEXT);
         CREATE TABLE data(id INTEGER, book INTEGER, format TEXT, name TEXT, uncompressed_size INTEGER);
         INSERT INTO books VALUES(1,'Locked','Author','author/book','2026-01-01','2000-01-01');
         INSERT INTO data VALUES(7,1,'EPUB','book',4);",
    )
    .unwrap();
    let config = root.join("config");
    configure_calibre(&config, vec![library]).unwrap();
    let original = fs::read(&db_path).unwrap();

    let locker = rusqlite::Connection::open(&db_path).unwrap();
    locker.execute_batch("BEGIN EXCLUSIVE").unwrap();
    let started = std::time::Instant::now();
    let error = calibre_list(&config, None, None, None).unwrap_err();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "a locked Calibre database must fail promptly"
    );
    assert_eq!(error.kind, boox_core::services::ServiceErrorKind::LocalIo);
    assert_eq!(error.message, "cannot query Calibre database");
    locker.execute_batch("ROLLBACK").unwrap();
    drop(locker);

    let books = calibre_list(&config, None, None, None).unwrap();
    assert_eq!(books.len(), 1);
    assert_eq!(books[0].title, "Locked");
    assert_eq!(fs::read(&db_path).unwrap(), original);
    drop(db);
    fs::remove_dir_all(root).unwrap();
}
