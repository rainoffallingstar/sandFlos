mod common;

use common::{
    command, command_with_api, json_stdout, run_with_stdin, stderr, stdout, ApiFault, ApiServer,
    TestDir,
};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::Output,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

const DOWNLOADED_BYTES: &[u8] = b"downloaded PDF payload";

fn assert_selected_file_is_pushed(dir: &TestDir, args: &[&str], bytes: &[u8]) {
    let server = ApiServer::start();
    let output = command_with_api(args, &dir.config_dir(), server.endpoint())
        .env("BOOXCTL_TOKEN", "fixture-token")
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

fn configure_zotero_without_local_file(dir: &TestDir) -> PathBuf {
    let library = dir.path().join("Zotero Data");
    fs::create_dir_all(&library).unwrap();
    let db = rusqlite::Connection::open(library.join("zotero.sqlite")).unwrap();
    db.execute_batch(
        "CREATE TABLE items(itemID INTEGER, key TEXT);
        CREATE TABLE itemAttachments(itemID INTEGER, path TEXT, linkMode INTEGER);
        INSERT INTO items VALUES(4,'ABCDEFGH');
        INSERT INTO itemAttachments VALUES(4,'storage:book.pdf',0);",
    )
    .unwrap();
    drop(db);
    configure(dir, "zotero", "--data-dir", &library);
    library
}

fn configure_webdav(dir: &TestDir, endpoint: &str) {
    let output = run_with_stdin(
        &[
            "--json",
            "zotero",
            "webdav",
            "configure",
            "--url",
            endpoint,
            "--username",
            "user",
            "--password-stdin",
        ],
        &dir.config_dir(),
        "test-password\n",
    );
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(json_stdout(&output)["data"]["configured"], true);
}

struct WebDavServer {
    endpoint: String,
    request: Arc<Mutex<Option<String>>>,
    handle: Option<thread::JoinHandle<()>>,
}

impl WebDavServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind WebDAV fixture");
        listener
            .set_nonblocking(true)
            .expect("nonblocking WebDAV fixture");
        let endpoint = format!("http://{}/zotero", listener.local_addr().unwrap());
        let request = Arc::new(Mutex::new(None));
        let observed = Arc::clone(&request);
        let handle = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => return,
                }
            };
            stream
                .set_nonblocking(false)
                .expect("blocking WebDAV connection");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("WebDAV read timeout");
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .expect("WebDAV write timeout");
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            loop {
                let mut line = String::new();
                assert!(
                    reader.read_line(&mut line).unwrap() > 0,
                    "incomplete WebDAV request"
                );
                let complete = line == "\r\n";
                request.push_str(&line);
                if complete {
                    break;
                }
            }
            *observed.lock().unwrap() = Some(request);

            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            zip.start_file("book.pdf", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(DOWNLOADED_BYTES).unwrap();
            let bytes = zip.finish().unwrap().into_inner();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            )
            .unwrap();
            stream.write_all(&bytes).unwrap();
        });
        Self {
            endpoint,
            request,
            handle: Some(handle),
        }
    }

    fn endpoint(&self) -> &str {
        &self.endpoint
    }

    fn request(&self) -> String {
        self.request
            .lock()
            .unwrap()
            .clone()
            .expect("WebDAV request was recorded")
    }
}

impl Drop for WebDavServer {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn push_with_download(dir: &TestDir, api: &ApiServer) -> Output {
    let mut command = command_with_api(
        &["--json", "zotero", "push", "--attachment-id", "4"],
        &dir.config_dir(),
        api.endpoint(),
    );
    command
        .env("BOOXCTL_TEST_OSS_BASE_URL", api.endpoint())
        .env("BOOXCTL_TOKEN", "fixture-token")
        .output()
        .expect("run Zotero push")
}

fn prepare_webdav_push(
    dir: &TestDir,
    api_fault: ApiFault,
) -> (WebDavServer, ApiServer, PathBuf, Vec<u8>) {
    let library = configure_zotero_without_local_file(dir);
    let db_path = library.join("zotero.sqlite");
    let original_db = fs::read(&db_path).unwrap();
    let webdav = WebDavServer::start();
    configure_webdav(dir, webdav.endpoint());
    let api = ApiServer::start_with_fault(api_fault);
    (webdav, api, db_path, original_db)
}

fn assert_downloaded_bytes_and_cleanup(output: &Output, api: &ApiServer, webdav: &WebDavServer) {
    let uploads = api
        .requests()
        .into_iter()
        .filter(|request| request.method == "PUT" && request.path.starts_with("/uid-fixture/push/"))
        .collect::<Vec<_>>();
    assert_eq!(uploads.len(), 1);
    assert_eq!(uploads[0].body.as_bytes(), DOWNLOADED_BYTES);

    let request = webdav.request();
    assert!(request.starts_with("GET /zotero/ABCDEFGH.zip HTTP/1.1"));
    let authorization = request
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        .map(|(_, value)| value.trim());
    assert_eq!(authorization, Some("Basic dXNlcjp0ZXN0LXBhc3N3b3Jk"));

    let value = json_stdout(output);
    let path = Path::new(value["data"]["files"][0]["path"].as_str().unwrap());
    assert_eq!(path.file_name().unwrap(), "book.pdf");
    assert!(!path.exists(), "downloaded attachment remains: {path:?}");
    let parent = path.parent().unwrap();
    assert!(!parent.exists(), "temporary directory remains: {parent:?}");
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

#[test]
fn zotero_webdav_attachment_downloads_then_pushes_and_cleans_up() {
    let dir = TestDir::new();
    let (webdav, api, db_path, original_db) = prepare_webdav_push(&dir, ApiFault::None);

    let output = push_with_download(&dir, &api);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout: {} stderr: {}",
        stdout(&output),
        stderr(&output)
    );
    let value = json_stdout(&output);
    assert_eq!(value["data"]["accepted"], 1);
    assert_eq!(value["data"]["files"][0]["outcome"], "accepted");

    assert_downloaded_bytes_and_cleanup(&output, &api, &webdav);
    assert_eq!(fs::read(db_path).unwrap(), original_db);
}

#[test]
fn zotero_webdav_download_is_cleaned_up_when_submission_is_unknown() {
    let dir = TestDir::new();
    let (webdav, api, db_path, original_db) = prepare_webdav_push(&dir, ApiFault::SaveAndPushFails);

    let output = push_with_download(&dir, &api);
    assert_eq!(output.status.code(), Some(6), "stdout: {}", stdout(&output));
    let value = json_stdout(&output);
    assert_eq!(value["data"]["accepted"], 0);
    assert_eq!(value["data"]["unknown"], 1);
    assert_eq!(value["data"]["files"][0]["outcome"], "unknown");
    assert_eq!(value["data"]["files"][0]["stage"], "save_and_push");
    assert_eq!(
        api.requests()
            .iter()
            .filter(|request| request.path == "/api/1/push/saveAndPush")
            .count(),
        1,
    );
    assert_downloaded_bytes_and_cleanup(&output, &api, &webdav);
    assert_eq!(fs::read(db_path).unwrap(), original_db);
}
