use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode},
};
use http_body_util::BodyExt;
use kapitel::{
    AppState, Config,
    auth::{NewProfile, insert_profile},
    index::Scanner,
};
use serde_json::{Value, json};
use tower::ServiceExt;

struct TestApp {
    state: AppState,
    router: Router,
    _directory: tempfile::TempDir,
}
#[derive(Clone)]
struct Login {
    cookie: String,
    csrf: String,
    id: String,
}
impl TestApp {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let library = directory.path().join("library");
        std::fs::create_dir(&library).unwrap();
        let state = kapitel::initialize(Config {
            library_directory: library,
            data_directory: directory.path().join("data"),
            public_origin: "http://localhost:8080".into(),
            secure_cookies: false,
            scan_interval_seconds: 30,
            inspector_executable: env!("CARGO_BIN_EXE_kapitel").into(),
        })
        .await
        .unwrap();
        insert_profile(
            &state,
            NewProfile {
                username: "owner".into(),
                password: "a synthetic test password".into(),
                is_admin: true,
            },
        )
        .await
        .unwrap();
        insert_profile(
            &state,
            NewProfile {
                username: "listener".into(),
                password: "a synthetic test password".into(),
                is_admin: false,
            },
        )
        .await
        .unwrap();
        let router = kapitel::router(state.clone()).await.unwrap();
        Self {
            state,
            router,
            _directory: directory,
        }
    }
    async fn login(&self, username: &str) -> Login {
        let request = Request::builder()
            .method("POST")
            .uri("/api/session")
            .header("origin", "http://localhost:8080")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"username":username,"password":"a synthetic test password"}).to_string(),
            ))
            .unwrap();
        let response = self.router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        let body: Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        Login {
            cookie,
            csrf: body["csrf_token"].as_str().unwrap().into(),
            id: body["profile"]["id"].as_str().unwrap().into(),
        }
    }
    async fn request(
        &self,
        login: Option<&Login>,
        method: &str,
        path: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("origin", "http://localhost:8080")
            .header("content-type", "application/json");
        if let Some(login) = login {
            request = request
                .header("cookie", &login.cookie)
                .header("x-csrf-token", &login.csrf);
        }
        let response = self
            .router
            .clone()
            .oneshot(request.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
    async fn book(&self) {
        std::fs::write(
            self.state.config.library_directory.join("audio.mp3"),
            b"0123456789",
        )
        .unwrap();
        let modified = kapitel::index::modified_ns(
            &std::fs::metadata(self.state.config.library_directory.join("audio.mp3")).unwrap(),
        );
        sqlx::query("INSERT INTO books(id,signature,title,has_audio,updated_at) VALUES('book','signature','Synthetic book',1,0)").execute(&self.state.database).await.unwrap();
        for id in ["part-a", "part-b"] {
            sqlx::query("INSERT INTO media_files(id,book_id,fingerprint,kind,content_type,duration_seconds,sort_order,title) VALUES(?,'book',?,'audio','audio/mpeg',120,0,?)").bind(id).bind(id).bind(id).execute(&self.state.database).await.unwrap();
        }
        sqlx::query("INSERT INTO file_locations VALUES('audio.mp3','part-a',10,?,1,'test')")
            .bind(modified)
            .execute(&self.state.database)
            .await
            .unwrap();
    }
    async fn start(&self, login: &Login) -> String {
        let (status, result) = self
            .request(
                Some(login),
                "POST",
                "/api/playback-sessions",
                json!({"book_id":"book"}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{result}");
        result["playback_session_id"].as_str().unwrap().into()
    }
    async fn save(
        &self,
        login: &Login,
        session: &str,
        sequence: i64,
        position: f64,
    ) -> (StatusCode, Value) {
        self.request(Some(login),"PUT",&format!("/api/playback-sessions/{session}/position"),json!({"media_file_id":"part-a","sequence_number":sequence,"playback_position_seconds":position})).await
    }
}

#[tokio::test]
async fn playback_handoff_preserves_rewinds_and_rejects_stale_devices() {
    let app = TestApp::new().await;
    app.book().await;
    let desktop = app.login("owner").await;
    let mobile = app.login("owner").await;
    let first = app.start(&desktop).await;
    assert_eq!(app.save(&desktop, &first, 1, 65.25).await.0, StatusCode::OK);
    let (status, resumed) = app
        .request(
            Some(&mobile),
            "POST",
            "/api/playback-sessions",
            json!({"book_id":"book"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(resumed["position"]["playback_position_seconds"], 65.25);
    let second = resumed["playback_session_id"].as_str().unwrap();
    assert_eq!(app.save(&mobile, second, 1, 35.5).await.0, StatusCode::OK);
    let (status, error) = app.save(&desktop, &first, 2, 90.0).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["error"]["code"], "session_replaced");
    let (_, book) = app
        .request(Some(&desktop), "GET", "/api/books/book", Value::Null)
        .await;
    assert_eq!(book["position"]["playback_position_seconds"], 35.5);
    let listener = app.login("listener").await;
    let (_, book) = app
        .request(Some(&listener), "GET", "/api/books/book", Value::Null)
        .await;
    assert!(book["position"].is_null());
}

#[tokio::test]
async fn sequence_numbers_are_atomic_and_retryable() {
    let app = TestApp::new().await;
    app.book().await;
    let login = app.login("owner").await;
    let session = app.start(&login).await;
    let (first, second) = tokio::join!(
        app.save(&login, &session, 1, 60.0),
        app.save(&login, &session, 2, 10.0)
    );
    assert_eq!(second.0, StatusCode::OK);
    assert!([StatusCode::OK, StatusCode::CONFLICT].contains(&first.0));
    assert_eq!(app.save(&login, &session, 2, 10.0).await.0, StatusCode::OK);
    assert_eq!(
        app.save(&login, &session, 2, 50.0).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.save(&login, &session, 3, 999.0).await.0,
        StatusCode::BAD_REQUEST
    );
    let (_, book) = app
        .request(Some(&login), "GET", "/api/books/book", Value::Null)
        .await;
    assert_eq!(book["position"]["playback_position_seconds"], 10.0);
}

#[tokio::test]
async fn authentication_csrf_and_revocation_protect_every_resource() {
    let app = TestApp::new().await;
    app.book().await;
    for path in [
        "/api/books",
        "/api/books/book",
        "/api/books/book/cover",
        "/api/media/part-a",
        "/api/admin/profiles",
        "/api/admin/scan",
    ] {
        assert_eq!(
            app.request(None, "GET", path, Value::Null).await.0,
            StatusCode::UNAUTHORIZED,
            "{path}"
        );
    }
    let owner = app.login("owner").await;
    let listener = app.login("listener").await;
    assert_eq!(
        app.request(Some(&listener), "GET", "/api/admin/profiles", Value::Null)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    for (origin, csrf) in [
        ("https://evil.example", owner.csrf.as_str()),
        ("http://localhost:8080", "wrong"),
    ] {
        let request = Request::builder()
            .method("POST")
            .uri("/api/admin/scan")
            .header("origin", origin)
            .header("cookie", &owner.cookie)
            .header("x-csrf-token", csrf)
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.router.clone().oneshot(request).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        app.request(
            Some(&owner),
            "PATCH",
            &format!("/api/admin/profiles/{}", owner.id),
            json!({"enabled":false})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.request(
            Some(&owner),
            "PATCH",
            &format!("/api/admin/profiles/{}", listener.id),
            json!({"password":"a different test password"})
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.request(Some(&listener), "GET", "/api/books", Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.request(Some(&owner), "DELETE", "/api/session", Value::Null)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.request(Some(&owner), "GET", "/api/books", Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn ranges_stream_exact_bytes_and_reject_changed_files() {
    let app = TestApp::new().await;
    app.book().await;
    let login = app.login("owner").await;
    for (method, range, status, length, body) in [
        (
            Method::GET,
            "bytes=2-5",
            StatusCode::PARTIAL_CONTENT,
            "4",
            "2345",
        ),
        (
            Method::HEAD,
            "bytes=2-5",
            StatusCode::PARTIAL_CONTENT,
            "4",
            "",
        ),
    ] {
        let request = Request::builder()
            .method(method)
            .uri("/api/media/part-a")
            .header("cookie", &login.cookie)
            .header("range", range)
            .body(Body::empty())
            .unwrap();
        let response = app.router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()["content-length"], length);
        assert_eq!(response.headers()["content-range"], "bytes 2-5/10");
        assert_eq!(
            &response.into_body().collect().await.unwrap().to_bytes()[..],
            body.as_bytes()
        );
    }
    std::fs::write(
        app.state.config.library_directory.join("audio.mp3"),
        b"replaced",
    )
    .unwrap();
    assert_eq!(
        app.request(Some(&login), "GET", "/api/media/part-a", Value::Null)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn indexing_preserves_identity_and_corrections_across_moves_and_missing_mounts() {
    let app = TestApp::new().await;
    let owner = app.login("owner").await;
    let directory = app.state.config.library_directory.join("Ebooks/Original");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("book.pdf"),
        b"%PDF-1.4\nsynthetic test content",
    )
    .unwrap();
    std::fs::write(
        directory.join("metadata.opf"),
        b"<metadata><title>Local title</title><creator>Writer</creator></metadata>",
    )
    .unwrap();
    let mut scanner = Scanner::default();
    scanner.scan(&app.state).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
        .fetch_one(&app.state.database)
        .await
        .unwrap();
    assert_eq!(count, 0);
    scanner.scan(&app.state).await.unwrap();
    let (_, books) = app
        .request(Some(&owner), "GET", "/api/books", Value::Null)
        .await;
    assert_eq!(books["total"], 1);
    let id = books["books"][0]["id"].as_str().unwrap();
    assert_eq!(books["books"][0]["title"], "Local title");
    assert_eq!(
        app.request(
            Some(&owner),
            "PATCH",
            &format!("/api/admin/books/{id}"),
            json!({"metadata":{"title":"My corrected title"}})
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let moved = app.state.config.library_directory.join("Renamed");
    std::fs::rename(&directory, &moved).unwrap();
    scanner.scan(&app.state).await.unwrap();
    scanner.scan(&app.state).await.unwrap();
    let (_, books) = app
        .request(Some(&owner), "GET", "/api/books", Value::Null)
        .await;
    assert_eq!(books["total"], 1);
    assert_eq!(books["books"][0]["id"], id);
    assert_eq!(books["books"][0]["title"], "My corrected title");
    let temporary = app
        .state
        .config
        .library_directory
        .join("temporarily-hidden.part");
    std::fs::rename(moved.join("book.pdf"), &temporary).unwrap();
    scanner.scan(&app.state).await.unwrap();
    assert!(
        !sqlx::query_scalar::<_, bool>("SELECT available FROM books WHERE id=?")
            .bind(id)
            .fetch_one(&app.state.database)
            .await
            .unwrap()
    );
    std::fs::rename(&temporary, moved.join("book.pdf")).unwrap();
    scanner.scan(&app.state).await.unwrap();
    scanner.scan(&app.state).await.unwrap();
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT available FROM books WHERE id=?")
            .bind(id)
            .fetch_one(&app.state.database)
            .await
            .unwrap()
    );
    let absent = app.state.config.library_directory.with_extension("offline");
    std::fs::rename(&app.state.config.library_directory, &absent).unwrap();
    assert!(scanner.scan(&app.state).await.is_err());
    let available: bool = sqlx::query_scalar("SELECT available FROM books WHERE id=?")
        .bind(id)
        .fetch_one(&app.state.database)
        .await
        .unwrap();
    assert!(available);
}

#[tokio::test]
async fn a_bad_book_does_not_prevent_other_books_from_indexing() {
    let app = TestApp::new().await;
    let root = &app.state.config.library_directory;
    std::fs::create_dir(root.join("Good")).unwrap();
    std::fs::create_dir(root.join("Bad")).unwrap();
    std::fs::write(root.join("Good/book.pdf"), b"%PDF-1.4\nvalid signature").unwrap();
    std::fs::write(root.join("Bad/book.mp3"), b"not audio").unwrap();
    let mut scanner = Scanner::default();
    scanner.scan(&app.state).await.unwrap();
    scanner.scan(&app.state).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM books")
            .fetch_one(&app.state.database)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM index_issues")
            .fetch_one(&app.state.database)
            .await
            .unwrap(),
        1
    );
}
