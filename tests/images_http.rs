//! images::Http against a local server, and the keyed fallback via linearctl.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::thread;

use linear_tui::ctl::Runner;
use linear_tui::images::{FetchError, Http, Loader, MAX_BYTES, Source};

/// Serve one canned response per connection, chosen by the request path.
fn serve() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut line = String::new();
            let mut r = BufReader::new(s.try_clone().unwrap());
            r.read_line(&mut line).unwrap();
            while r.read_line(&mut String::new()).map(|n| n > 2).unwrap_or(false) {}
            let path = line.split(' ').nth(1).unwrap_or("/").to_string();
            let (status, body): (&str, Vec<u8>) = match path.as_str() {
                "/ok" => ("200 OK", b"image bytes".to_vec()),
                "/expired" => ("401 Unauthorized", Vec::new()),
                "/forbidden" => ("403 Forbidden", Vec::new()),
                "/moved" => ("302 Found", Vec::new()),
                "/huge" => ("200 OK", vec![0; MAX_BYTES as usize + 10]),
                _ => ("500 Internal Server Error", Vec::new()),
            };
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nLocation: http://example.com/\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = s.write_all(head.as_bytes());
            let _ = s.write_all(&body);
        }
    });
    format!("http://{addr}")
}

#[test]
fn http_statuses() {
    let base = serve();
    let http = Http::with_https_only(false);
    assert_eq!(http.fetch(&format!("{base}/ok")).unwrap(), b"image bytes");
    assert_eq!(http.fetch(&format!("{base}/expired")), Err(FetchError::Expired));
    assert_eq!(http.fetch(&format!("{base}/forbidden")), Err(FetchError::Expired));
    assert_eq!(
        http.fetch(&format!("{base}/moved")),
        Err(FetchError::Status(302)),
        "no redirects"
    );
    assert_eq!(http.fetch(&format!("{base}/boom")), Err(FetchError::Status(500)));
    assert_eq!(http.fetch(&format!("{base}/huge")), Err(FetchError::TooLarge));
}

#[test]
fn http_refuses_plain_http_by_default() {
    let base = serve();
    assert!(matches!(
        Http::new().fetch(&format!("{base}/ok")),
        Err(FetchError::Network(_))
    ));
    assert!(matches!(
        Http::default().fetch("http://127.0.0.1:1/x"),
        Err(FetchError::Network(_))
    ));
}

struct Expired;
impl Source for Expired {
    fn fetch(&self, _: &str) -> Result<Vec<u8>, FetchError> {
        Err(FetchError::Expired)
    }
}

#[test]
fn an_expired_link_falls_back_to_linearctl_file_download() {
    let loader = Loader {
        source: Box::new(Expired),
        cache: None,
        fallback: Some(Runner::new(env!("CARGO_BIN_EXE_fake-linearctl"))),
    };
    // The fake refuses writes, file download included: the error comes through.
    let err = loader
        .load("https://uploads.linear.app/demo/checkout-error.png?signature=old")
        .unwrap_err();
    assert_eq!(err, "the demo workspace is read-only");
}
