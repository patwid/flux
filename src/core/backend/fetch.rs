//! Downloading a stream to a file in byte ranges, retrying failed requests.

use std::path::Path;

use tokio::io::AsyncWriteExt;

use super::{BackendError, Result};
use crate::core::download::{self, RetryPolicy};

/// How [`fetch`] requests a stream.
#[derive(Debug, Clone)]
pub struct Options {
    /// Size of the byte ranges requested.
    pub chunk: u64,
    pub retry: RetryPolicy,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            chunk: download::CHUNK_SIZE,
            retry: RetryPolicy::default(),
        }
    }
}

/// Why a range request failed.
struct Failure {
    error: BackendError,
    retryable: bool,
}

impl Failure {
    fn fatal(message: String) -> Self {
        Failure {
            error: BackendError::Other(message),
            retryable: false,
        }
    }
}

impl From<reqwest::Error> for Failure {
    /// Timeouts and broken connections are worth retrying; a status is
    /// retryable if [`download::retryable_status`] says so. 403 and 410 mean
    /// the URL was refused.
    fn from(e: reqwest::Error) -> Self {
        if let Some(403 | 410) = e.status().map(|s| s.as_u16()) {
            return Failure {
                error: BackendError::UrlRefused(e.to_string()),
                retryable: false,
            };
        }
        let retryable = match e.status() {
            Some(status) => download::retryable_status(status.as_u16()),
            // A body cut short is a decode error.
            None => {
                e.is_timeout() || e.is_connect() || e.is_request() || e.is_body() || e.is_decode()
            }
        };
        Failure {
            error: e.into(),
            retryable,
        }
    }
}

impl From<std::io::Error> for Failure {
    fn from(e: std::io::Error) -> Self {
        Failure {
            error: e.into(),
            retryable: false,
        }
    }
}

/// Downloads `url` (of `size` bytes) to `path` in ranges of `options.chunk`
/// bytes, continuing after the bytes already in `path`. A failed request is retried from the last byte received, waiting
/// as `options.retry` says; the retries start over once bytes arrive again.
pub async fn fetch(
    http: &reqwest::Client,
    url: &str,
    size: u64,
    path: &Path,
    options: &Options,
    on_bytes: &mut impl FnMut(u64),
) -> Result<()> {
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    let mut offset = file.metadata().await?.len();
    if offset > size {
        file.set_len(0).await?;
        offset = 0;
    }
    on_bytes(offset);
    let mut retry = 0;
    while let Some(range) = download::next_range(offset, size, options.chunk) {
        let before = offset;
        let result = fetch_range(http, url, range, &mut file, &mut offset, on_bytes).await;
        if offset > before {
            retry = 0;
        }
        if let Err(failure) = result {
            match options.retry.delay(retry).filter(|_| failure.retryable) {
                Some(delay) => {
                    retry += 1;
                    tokio::time::sleep(delay).await;
                }
                None => return Err(failure.error),
            }
        }
    }
    file.flush().await?;
    Ok(())
}

/// Requests the inclusive byte `range` of `url` and appends it to `file`,
/// advancing `offset` by the bytes written.
async fn fetch_range(
    http: &reqwest::Client,
    url: &str,
    (start, end): (u64, u64),
    file: &mut tokio::fs::File,
    offset: &mut u64,
    on_bytes: &mut impl FnMut(u64),
) -> std::result::Result<(), Failure> {
    let mut response = http
        .get(url)
        .header("Range", format!("bytes={start}-{end}"))
        .send()
        .await?
        .error_for_status()?;
    let content_start = response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|h| h.to_str().ok())
        .and_then(download::content_range_start);
    if response.status() != reqwest::StatusCode::PARTIAL_CONTENT || content_start != Some(start) {
        return Err(Failure::fatal(format!(
            "the server ignored the byte range {start}-{end} (status {})",
            response.status()
        )));
    }
    while let Some(chunk) = response.chunk().await? {
        // Never write past the range, whatever the server sends.
        let wanted = (end + 1 - *offset).min(chunk.len() as u64) as usize;
        file.write_all(&chunk[..wanted]).await?;
        *offset += wanted as u64;
        on_bytes(wanted as u64);
    }
    if *offset <= end {
        return Err(Failure {
            error: BackendError::Other(format!("connection closed at byte {offset}")),
            retryable: true,
        });
    }
    Ok(())
}

/// Makes the request of `attempt` again, waiting as `policy` says, while
/// it fails in a way worth retrying (see [`download::retryable_status`]).
pub async fn with_retries<T, F, Fut>(policy: &RetryPolicy, mut attempt: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = reqwest::Result<T>>,
{
    let mut retry = 0;
    loop {
        let failure = match attempt().await {
            Ok(value) => return Ok(value),
            Err(e) => Failure::from(e),
        };
        match policy.delay(retry).filter(|_| failure.retryable) {
            Some(delay) => {
                retry += 1;
                tokio::time::sleep(delay).await;
            }
            None => return Err(failure.error),
        }
    }
}

/// The size of the stream at `url`, from the `Content-Range` of a request
/// for its first byte.
pub async fn size(http: &reqwest::Client, url: &str) -> Result<u64> {
    let response = http
        .get(url)
        .header("Range", "bytes=0-0")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| Failure::from(e).error)?;
    response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|h| h.to_str().ok())
        .and_then(download::content_range_total)
        .ok_or_else(|| BackendError::Other("unknown stream size".into()))
}

/// Whether `url` (of `size` bytes) serves the middle of the stream, as some
/// clients' URLs only serve its start, or none of it. Only a refusal (see
/// [`BackendError::UrlRefused`]) counts: other failures may be passing.
pub async fn serves(http: &reqwest::Client, url: &str, size: u64) -> bool {
    let middle = size / 2;
    let response = http
        .get(url)
        .header("Range", format!("bytes={middle}-{middle}"))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status);
    match response {
        Ok(_) => true,
        Err(e) => !matches!(Failure::from(e).error, BackendError::UrlRefused(_)),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    /// How the fake server answers a request.
    #[derive(Clone, Copy)]
    enum Reply {
        /// The requested range.
        Range,
        /// The requested range, but the connection closes after `n` bytes.
        Truncated(usize),
        /// The headers and `n` bytes of the range, then nothing.
        Stall(usize),
        /// An empty response with this status.
        Status(u16),
        /// The whole file with status 200, ignoring the range.
        Whole,
    }

    /// A server for `data` answering requests as `script` says, then with
    /// [`Reply::Range`]. Returns its URL and the ranges requested.
    async fn serve(data: Vec<u8>, script: Vec<Reply>) -> (String, Arc<Mutex<Vec<(u64, u64)>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/stream", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = requests.clone();
        tokio::spawn(async move {
            let mut script = script.into_iter();
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut head = Vec::new();
                while !head.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    if socket.read(&mut byte).await.unwrap() == 0 {
                        break;
                    }
                    head.push(byte[0]);
                }
                let head = String::from_utf8(head).unwrap();
                let (start, end) = head
                    .lines()
                    .find_map(|l| l.strip_prefix("range: bytes="))
                    .and_then(|r| r.split_once('-'))
                    .map(|(s, e)| (s.parse::<u64>().unwrap(), e.parse::<u64>().unwrap()))
                    .expect("a range request");
                log.lock().unwrap().push((start, end));
                let body = &data[start as usize..=end as usize];
                let partial = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    data.len(),
                    body.len()
                );
                let reply = script.next().unwrap_or(Reply::Range);
                let mut response = match reply {
                    Reply::Range | Reply::Truncated(_) | Reply::Stall(_) => partial.into_bytes(),
                    Reply::Status(status) => format!(
                        "HTTP/1.1 {status} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .into_bytes(),
                    Reply::Whole => format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        data.len()
                    )
                    .into_bytes(),
                };
                match reply {
                    Reply::Range => response.extend_from_slice(body),
                    Reply::Truncated(n) | Reply::Stall(n) => response.extend_from_slice(&body[..n]),
                    Reply::Status(_) => {}
                    Reply::Whole => response.extend_from_slice(&data),
                }
                let _ = socket.write_all(&response).await;
                if let Reply::Stall(_) = reply {
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                        drop(socket);
                    });
                }
            }
        });
        (url, requests)
    }

    fn data() -> Vec<u8> {
        (0..25u8).collect()
    }

    fn options(retries: u32) -> Options {
        Options {
            chunk: 10,
            retry: RetryPolicy {
                retries,
                first_delay: Duration::ZERO,
                max_delay: Duration::ZERO,
            },
        }
    }

    fn client() -> reqwest::Client {
        reqwest::Client::builder()
            .no_proxy()
            .read_timeout(Duration::from_millis(200))
            .build()
            .unwrap()
    }

    /// Fetches the fake stream with `script` and `retries`, returning the
    /// result, the file and the bytes reported.
    async fn run(script: Vec<Reply>, retries: u32) -> (Result<()>, Vec<u8>, u64, Vec<(u64, u64)>) {
        resume(&[], script, retries).await
    }

    /// Like [`run`], with the file already holding `part`.
    async fn resume(
        part: &[u8],
        script: Vec<Reply>,
        retries: u32,
    ) -> (Result<()>, Vec<u8>, u64, Vec<(u64, u64)>) {
        let (url, requests) = serve(data(), script).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stream.part");
        if !part.is_empty() {
            std::fs::write(&path, part).unwrap();
        }
        let mut reported = 0;
        let result = fetch(
            &client(),
            &url,
            data().len() as u64,
            &path,
            &options(retries),
            &mut |n| reported += n,
        )
        .await;
        let file = std::fs::read(&path).unwrap_or_default();
        let requests = requests.lock().unwrap().clone();
        (result, file, reported, requests)
    }

    #[tokio::test]
    async fn fetches_in_ranges() {
        let (result, file, reported, requests) = run(vec![], 0).await;
        result.unwrap();
        assert_eq!(file, data());
        assert_eq!(reported, 25);
        assert_eq!(requests, [(0, 9), (10, 19), (20, 24)]);
    }

    #[tokio::test]
    async fn retries_server_errors() {
        let (result, file, _, requests) = run(
            vec![Reply::Range, Reply::Status(503), Reply::Status(429)],
            2,
        )
        .await;
        result.unwrap();
        assert_eq!(file, data());
        assert_eq!(requests, [(0, 9), (10, 19), (10, 19), (10, 19), (20, 24)]);
    }

    #[tokio::test]
    async fn gives_up_after_the_retries() {
        let script = vec![Reply::Status(503); 3];
        let (result, _, _, requests) = run(script, 2).await;
        assert!(result.unwrap_err().to_string().contains("503"));
        assert_eq!(requests.len(), 3);
    }

    #[tokio::test]
    async fn does_not_retry_client_errors() {
        let (result, _, _, requests) = run(vec![Reply::Status(403)], 5).await;
        assert!(result.unwrap_err().to_string().contains("403"));
        assert_eq!(requests.len(), 1);
    }

    #[tokio::test]
    async fn continues_after_the_last_byte_received() {
        let (result, file, reported, requests) = run(vec![Reply::Truncated(4)], 1).await;
        result.unwrap();
        assert_eq!(file, data());
        assert_eq!(reported, 25);
        assert_eq!(requests, [(0, 9), (4, 13), (14, 23), (24, 24)]);
    }

    #[tokio::test]
    async fn retries_stalled_responses() {
        let (result, file, _, requests) = run(vec![Reply::Range, Reply::Stall(3)], 1).await;
        result.unwrap();
        assert_eq!(file, data());
        assert_eq!(requests[..3], [(0, 9), (10, 19), (13, 22)]);
    }

    #[tokio::test]
    async fn progress_resets_the_retries() {
        // Each failure still delivers bytes, so one retry is always enough.
        let script = vec![Reply::Truncated(2); 5];
        let (result, file, _, _) = run(script, 1).await;
        result.unwrap();
        assert_eq!(file, data());
    }

    #[tokio::test]
    async fn resumes_a_partial_file() {
        let (result, file, reported, requests) = resume(&data()[..13], vec![], 0).await;
        result.unwrap();
        assert_eq!(file, data());
        assert_eq!(reported, 25, "the resumed bytes count as progress");
        assert_eq!(requests, [(13, 22), (23, 24)]);
    }

    #[tokio::test]
    async fn complete_files_are_not_fetched_again() {
        let (result, file, _, requests) = resume(&data(), vec![], 0).await;
        result.unwrap();
        assert_eq!(file, data());
        assert!(requests.is_empty());
    }

    #[tokio::test]
    async fn restarts_files_longer_than_the_stream() {
        let (result, file, reported, requests) = resume(&[7; 30], vec![], 0).await;
        result.unwrap();
        assert_eq!(file, data());
        assert_eq!(reported, 25);
        assert_eq!(requests[0], (0, 9));
    }

    #[tokio::test]
    async fn reports_refused_urls() {
        for status in [403, 410] {
            let (result, _, _, _) = run(vec![Reply::Status(status)], 5).await;
            let error = result.unwrap_err();
            assert!(matches!(error, BackendError::UrlRefused(_)), "{error:?}");
            assert!(error.to_string().contains(&status.to_string()), "{error}");
        }
    }

    /// Whether the fake stream with `script` [`serves`], with the range asked.
    async fn check(script: Vec<Reply>) -> (bool, Vec<(u64, u64)>) {
        let (url, requests) = serve(data(), script).await;
        let served = serves(&client(), &url, data().len() as u64).await;
        let requests = requests.lock().unwrap().clone();
        (served, requests)
    }

    /// Requests the first range of the fake stream with `script`, retrying
    /// `retries` times. Returns the result and the number of requests.
    async fn request(script: Vec<Reply>, retries: u32) -> (Result<Vec<u8>>, usize) {
        let (url, requests) = serve(data(), script).await;
        let http = client();
        let result = with_retries(&options(retries).retry, || async {
            let response = http
                .get(&url)
                .header("Range", "bytes=0-9")
                .send()
                .await?
                .error_for_status()?;
            Ok(response.bytes().await?.to_vec())
        })
        .await;
        let count = requests.lock().unwrap().len();
        (result, count)
    }

    #[tokio::test]
    async fn retries_requests() {
        let (result, count) = request(vec![Reply::Status(503), Reply::Stall(2)], 2).await;
        assert_eq!(result.unwrap(), data()[..10]);
        assert_eq!(count, 3);

        let (result, count) = request(vec![Reply::Status(503); 3], 2).await;
        assert!(result.is_err());
        assert_eq!(count, 3);

        let (result, count) = request(vec![Reply::Status(403)], 2).await;
        assert!(matches!(result, Err(BackendError::UrlRefused(_))));
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn finds_the_size_of_a_stream() {
        let (url, requests) = serve(data(), vec![]).await;
        assert_eq!(size(&client(), &url).await.unwrap(), 25);
        assert_eq!(*requests.lock().unwrap(), [(0, 0)]);

        let (url, _) = serve(data(), vec![Reply::Status(403)]).await;
        let error = size(&client(), &url).await.unwrap_err();
        assert!(matches!(error, BackendError::UrlRefused(_)), "{error:?}");

        let (url, _) = serve(data(), vec![Reply::Whole]).await;
        assert!(size(&client(), &url).await.is_err());
    }

    #[tokio::test]
    async fn checks_that_the_middle_of_a_stream_is_served() {
        assert_eq!(check(vec![]).await, (true, vec![(12, 12)]));
        assert!(!check(vec![Reply::Status(403)]).await.0);
        assert!(!check(vec![Reply::Status(410)]).await.0);
        assert!(check(vec![Reply::Status(503)]).await.0, "may be passing");
    }

    #[tokio::test]
    async fn rejects_responses_ignoring_the_range() {
        let (result, _, _, requests) = run(vec![Reply::Whole], 5).await;
        assert!(result.unwrap_err().to_string().contains("range"));
        assert_eq!(requests.len(), 1);
    }
}
