//! rv.dev/ruby — precompiled rubies for rv, served from a Fastly Object Storage bucket.
//!
//! A path under `/ruby/` is the key of an object in the bucket. The exception is a dateless
//! alias like `/ruby/3.4.11/macos-aarch64`, which serves the newest build for that platform.

use std::collections::HashMap;
use std::time::Duration;

use fastly::http::{header, FramingHeadersMode, HeaderValue, Method, StatusCode};
use fastly::secret_store::SecretStore;
use fastly::{Backend, Error, Request, Response};
use serde::Deserialize;
use time::OffsetDateTime;

use crate::sigv4;

/// The bucket's regional endpoint, like `us-central-1.object.fastlystorage.app`.
const BACKEND: &str = "rv_ruby";
const BUCKET: &str = "rv-ruby";
/// Holds the bucket's `access_key_id` and `secret_access_key`.
const SECRET_STORE: &str = "rv_ruby";
const BASE_URL: &str = "https://rv.dev/ruby/";

/// How long Fastly and its clients may keep a response.
struct Caching {
    ttl: u32,
    stale_while_revalidate: u32,
    cache_control: &'static str,
}

/// A build never changes once it is published.
const IMMUTABLE: Caching = Caching {
    ttl: 31_536_000,
    stale_while_revalidate: 0,
    cache_control: "public, max-age=31536000, immutable",
};

/// The index and the alias maps change with a release, which also purges them.
const SHORT: Caching = Caching {
    ttl: 60,
    stale_while_revalidate: 600,
    cache_control: "public, max-age=60, stale-while-revalidate=600",
};

#[derive(Debug, PartialEq)]
enum Route<'a> {
    /// `v1/index.json` or `v1/SHA256SUMS`
    Index(&'a str),
    /// `alias/{engine}/{version}.json`
    AliasMap { engine: &'a str, version: &'a str },
    /// `{version}/{build}/{file}`, or `{engine}/{version}/{build}/{file}` for another engine
    Artifact(&'a str),
    /// `{version}/{platform}`, or `{engine}/{version}/{platform}` for another engine
    Alias {
        engine: &'a str,
        version: &'a str,
        platform: &'a str,
    },
}

/// One platform's entry in an alias map, as written by `tools/index` in rv-ruby.
#[derive(Debug, Deserialize)]
struct AliasEntry {
    build: String,
    filename: String,
    url: String,
    size: u64,
    sha256: String,
}

#[derive(Debug, Deserialize)]
struct AliasMap {
    platforms: HashMap<String, AliasEntry>,
}

pub fn handle(req: &Request) -> Response {
    if !matches!(req.get_method(), &Method::GET | &Method::HEAD) {
        return Response::from_status(StatusCode::METHOD_NOT_ALLOWED)
            .with_header(header::ALLOW, "GET, HEAD")
            .with_body_text_plain("This method is not allowed\n");
    }

    let path = req.get_path().strip_prefix("/ruby/").unwrap_or_default();
    serve(req, path).unwrap_or_else(|err| {
        eprintln!("could not serve /ruby/{path}: {err:#}");
        unavailable()
    })
}

/// Is the bucket reachable, and does it have an index?
pub fn health() -> Response {
    match fetch(&Method::GET, None, "v1/index.json", None, None) {
        Ok(resp) if resp.get_status() == StatusCode::OK => Response::from_body("ok\n"),
        Ok(resp) => {
            eprintln!("the bucket answered {} for the index", resp.get_status());
            unavailable()
        }
        Err(err) => {
            eprintln!("could not reach the bucket: {err:#}");
            unavailable()
        }
    }
}

fn serve(req: &Request, path: &str) -> Result<Response, Error> {
    let method = req.get_method();
    let range = req.get_header(header::RANGE);

    match route(path) {
        Some(Route::Index(key)) => {
            let object = fetch(method, range, key, Some(&SHORT), Some("index"))?;
            Ok(compressed(respond(object, &SHORT)))
        }
        Some(Route::AliasMap { engine, version }) => {
            let object = fetch_alias_map(method, engine, version)?;
            Ok(compressed(respond(object, &SHORT)))
        }
        Some(Route::Artifact(key)) => {
            let object = fetch(method, range, key, Some(&IMMUTABLE), None)?;
            Ok(respond(object, &IMMUTABLE))
        }
        Some(Route::Alias {
            engine,
            version,
            platform,
        }) => {
            let mut map = fetch_alias_map(&Method::GET, engine, version)?;
            match map.get_status() {
                StatusCode::OK => {}
                StatusCode::NOT_FOUND => return Ok(not_found()),
                _ => return Ok(unavailable()),
            }
            let map: AliasMap = map.take_body_json()?;
            let Some(entry) = map.platforms.get(platform) else {
                return Ok(not_found());
            };
            let key = entry
                .url
                .strip_prefix(BASE_URL)
                .ok_or_else(|| Error::msg(format!("{} is not under {BASE_URL}", entry.url)))?;

            let object = fetch(method, range, key, Some(&IMMUTABLE), None)?;
            let resp = respond(object, &SHORT);
            if !resp.get_status().is_success() {
                return Ok(resp);
            }
            let size = body_size(&resp);
            if size != Some(entry.size) {
                eprintln!(
                    "{key} is {size:?}, but the alias map says {} bytes",
                    entry.size
                );
                return Ok(not_found());
            }
            Ok(entry.describe(resp))
        }
        None => Ok(not_found()),
    }
}

fn route(path: &str) -> Option<Route<'_>> {
    let segments: Vec<&str> = path.split('/').collect();
    if segments.iter().any(|s| s.is_empty() || s.starts_with('.')) {
        return None;
    }

    match segments.as_slice() {
        ["v1", "index.json" | "SHA256SUMS"] => Some(Route::Index(path)),
        ["v1", ..] => None,
        ["alias", engine, file] => file
            .strip_suffix(".json")
            .map(|version| Route::AliasMap { engine, version }),
        ["alias", ..] => None,
        [version, platform] => Some(Route::Alias {
            engine: "ruby",
            version,
            platform,
        }),
        ["jruby", version, platform] => Some(Route::Alias {
            engine: "jruby",
            version,
            platform,
        }),
        [_, _, _] | ["jruby", _, _, _] => Some(Route::Artifact(path)),
        _ => None,
    }
}

fn fetch_alias_map(method: &Method, engine: &str, version: &str) -> Result<Response, Error> {
    let key = format!("alias/{engine}/{version}.json");
    let surrogate_key = format!("alias:{engine}:{version}");
    fetch(method, None, &key, Some(&SHORT), Some(&surrogate_key))
}

/// Gets an object from the bucket. Without `caching`, the request always reaches the bucket.
///
/// The bucket only ever sees a `GET`, because that is what Fastly sends to fill its cache
/// whatever the client asked for, and the method is part of what gets signed.
fn fetch(
    method: &Method,
    range: Option<&HeaderValue>,
    key: &str,
    caching: Option<&Caching>,
    surrogate_key: Option<&str>,
) -> Result<Response, Error> {
    let backend = Backend::from_name(BACKEND)?;
    if !backend.exists() {
        return Err(Error::msg(format!("this service has no {BACKEND} backend")));
    }
    let host = backend.get_host();
    let region = host.split('.').next().unwrap_or_default();
    let path = format!("/{BUCKET}/{}", encode(key));
    let amz_date = amz_date(OffsetDateTime::now_utc());
    let target = sigv4::Target {
        method: "GET",
        host: &host,
        path: &path,
        query: "",
        region,
    };
    let authorization = sigv4::authorization(&credentials()?, &target, &amz_date);

    let mut bereq = Request::get(format!("https://{host}{path}"))
        .with_header("x-amz-date", &amz_date)
        .with_header("x-amz-content-sha256", sigv4::content_sha256())
        .with_header(header::AUTHORIZATION, authorization);
    if let Some(range) = range {
        bereq.set_header(header::RANGE, range);
    }
    match caching {
        Some(caching) => {
            bereq.set_stale_while_revalidate(caching.stale_while_revalidate);
            let ttl = Duration::from_secs(caching.ttl.into());
            bereq.set_after_send(move |candidate| {
                if candidate.get_status().is_success() {
                    candidate.set_ttl(ttl);
                } else {
                    candidate.set_ttl(Duration::from_mins(10));
                    candidate.set_body_transform(|_body, _out| Ok(()));
                };
                Ok(())
            });
        }
        None => bereq.set_pass(true),
    }
    if let Some(surrogate_key) = surrogate_key {
        bereq.set_surrogate_key(HeaderValue::from_str(surrogate_key)?);
    }

    let mut beresp = bereq.send(backend)?;
    if method == Method::HEAD {
        // Drop the body, and keep the Content-Length it would have had.
        beresp.take_body();
        beresp.set_framing_headers_mode(FramingHeadersMode::ManuallyFromHeaders);
    }
    Ok(beresp)
}

fn credentials() -> Result<sigv4::Credentials, Error> {
    let store = SecretStore::open(SECRET_STORE)?;
    let secret = |name: &str| -> Result<String, Error> {
        let secret = store
            .try_get(name)?
            .ok_or_else(|| Error::msg(format!("{SECRET_STORE} has no {name}")))?;
        Ok(String::from_utf8(secret.try_plaintext()?.to_vec())?)
    };

    Ok(sigv4::Credentials {
        access_key_id: secret("access_key_id")?,
        secret_access_key: secret("secret_access_key")?,
    })
}

/// Turns what the bucket said into what the client should see.
fn respond(mut beresp: Response, caching: &Caching) -> Response {
    match beresp.get_status() {
        StatusCode::OK | StatusCode::PARTIAL_CONTENT | StatusCode::RANGE_NOT_SATISFIABLE => {}
        StatusCode::NOT_FOUND => return not_found(),
        status => {
            eprintln!("the bucket answered {status}");
            return unavailable();
        }
    }

    let amz_headers: Vec<_> = beresp
        .get_header_names()
        .filter(|name| name.as_str().starts_with("x-amz-"))
        .cloned()
        .collect();
    for name in amz_headers {
        beresp.remove_header(name);
    }
    beresp.with_header(header::CACHE_CONTROL, caching.cache_control)
}

fn body_size(resp: &Response) -> Option<u64> {
    if resp.get_status() == StatusCode::PARTIAL_CONTENT {
        let range = resp.get_header_str(header::CONTENT_RANGE)?;
        range.rsplit('/').next()?.trim().parse().ok()
    } else {
        resp.get_content_length().map(|size| size as u64)
    }
}

/// Asks Fastly to compress the response for clients that accept it.
fn compressed(resp: Response) -> Response {
    resp.with_header("x-compress-hint", "on")
}

impl AliasEntry {
    /// Tells the client which build it got, since the URL it asked for doesn't say.
    fn describe(&self, resp: Response) -> Response {
        resp.with_header("x-rv-build", &self.build)
            .with_header("x-rv-sha256", &self.sha256)
            .with_header("x-rv-size", self.size.to_string())
            .with_header("x-rv-filename", &self.filename)
            .with_header(
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", self.filename),
            )
            .with_header(header::ACCEPT_RANGES, "bytes")
    }
}

fn not_found() -> Response {
    Response::from_status(StatusCode::NOT_FOUND)
        .with_body_text_plain("The page you requested could not be found.\n")
}

fn unavailable() -> Response {
    Response::from_status(StatusCode::SERVICE_UNAVAILABLE)
        .with_header(header::RETRY_AFTER, "30")
        .with_body_text_plain("rv.dev/ruby is unavailable right now. Please try again shortly.\n")
}

/// Percent-encodes an object key for a URL path, keeping its slashes.
fn encode(key: &str) -> String {
    key.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                char::from(byte).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// The `x-amz-date` header for a time, like `20130524T000000Z`.
fn amz_date(time: OffsetDateTime) -> String {
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        time.year(),
        u8::from(time.month()),
        time.day(),
        time.hour(),
        time.minute(),
        time.second()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALIAS_MAP: &str = r#"{
        "engine": "ruby",
        "version": "3.4.11",
        "platforms": {
            "macos-aarch64": {
                "build": "20261014",
                "filename": "ruby-3.4.11.macos-aarch64.tar.gz",
                "url": "https://rv.dev/ruby/3.4.11/20261014/ruby-3.4.11.macos-aarch64.tar.gz",
                "size": 36656127,
                "sha256": "0f2d7e8c6b5a49382716f5e4d3c2b1a00f2d7e8c6b5a49382716f5e4d3c2b1a0"
            }
        }
    }"#;

    #[test]
    fn routes_the_index_and_checksums() {
        assert_eq!(route("v1/index.json"), Some(Route::Index("v1/index.json")));
        assert_eq!(route("v1/SHA256SUMS"), Some(Route::Index("v1/SHA256SUMS")));
        assert_eq!(route("v1/index.json.prev"), None);
        assert_eq!(route("v1"), None);
    }

    #[test]
    fn routes_alias_maps() {
        assert_eq!(
            route("alias/ruby/4.1.0-preview1.json"),
            Some(Route::AliasMap {
                engine: "ruby",
                version: "4.1.0-preview1"
            })
        );
        assert_eq!(route("alias/ruby/3.4.11"), None);
        assert_eq!(route("alias/ruby"), None);
    }

    #[test]
    fn routes_artifacts_by_their_key() {
        let cruby = "3.4.11/20261014/ruby-3.4.11.macos-aarch64.tar.gz";
        let sidecar = "3.4.11/20261014/ruby-3.4.11.macos-aarch64.tar.gz.sha256";
        let jruby = "jruby/10.1.2.0/20261015/jruby-bin-10.1.2.0.tar.gz";

        assert_eq!(route(cruby), Some(Route::Artifact(cruby)));
        assert_eq!(route(sidecar), Some(Route::Artifact(sidecar)));
        assert_eq!(route(jruby), Some(Route::Artifact(jruby)));
    }

    #[test]
    fn routes_dateless_aliases() {
        assert_eq!(
            route("3.4.11/macos-aarch64"),
            Some(Route::Alias {
                engine: "ruby",
                version: "3.4.11",
                platform: "macos-aarch64"
            })
        );
        assert_eq!(
            route("dev/linux-musl-x86_64"),
            Some(Route::Alias {
                engine: "ruby",
                version: "dev",
                platform: "linux-musl-x86_64"
            })
        );
        assert_eq!(
            route("jruby/10.1.2.0/macos-x86_64"),
            Some(Route::Alias {
                engine: "jruby",
                version: "10.1.2.0",
                platform: "macos-x86_64"
            })
        );
    }

    #[test]
    fn routes_nothing_else() {
        assert_eq!(route(""), None);
        assert_eq!(route("3.4.11"), None);
        assert_eq!(route("3.4.11//macos-aarch64"), None);
        assert_eq!(route("3.4.11/../v1/index.json.prev"), None);
        assert_eq!(route("3.4.11/20261014/ruby/extra/segments"), None);
    }

    #[test]
    fn encodes_keys_for_the_bucket() {
        assert_eq!(
            encode("3.4.11/20261014/ruby-3.4.11.macos-aarch64.tar.gz"),
            "3.4.11/20261014/ruby-3.4.11.macos-aarch64.tar.gz"
        );
        assert_eq!(encode("a b/c+d"), "a%20b/c%2Bd");
    }

    #[test]
    fn formats_the_date_header() {
        let time = OffsetDateTime::from_unix_timestamp(1_369_353_600).unwrap();
        assert_eq!(amz_date(time), "20130524T000000Z");
    }

    #[test]
    fn describes_an_alias_response() {
        let map: AliasMap = serde_json::from_str(ALIAS_MAP).unwrap();
        let entry = &map.platforms["macos-aarch64"];
        let resp = entry.describe(Response::new());

        assert_eq!(resp.get_header_str("x-rv-build"), Some("20261014"));
        assert_eq!(resp.get_header_str("x-rv-size"), Some("36656127"));
        assert_eq!(
            resp.get_header_str("x-rv-sha256"),
            Some("0f2d7e8c6b5a49382716f5e4d3c2b1a00f2d7e8c6b5a49382716f5e4d3c2b1a0")
        );
        assert_eq!(
            resp.get_header_str("x-rv-filename"),
            Some("ruby-3.4.11.macos-aarch64.tar.gz")
        );
        assert_eq!(
            resp.get_header_str("Content-Disposition"),
            Some("attachment; filename=\"ruby-3.4.11.macos-aarch64.tar.gz\"")
        );
        assert_eq!(resp.get_header_str("Accept-Ranges"), Some("bytes"));
    }

    #[test]
    fn passes_an_object_through_with_our_caching() {
        let object = Response::from_body("a ruby")
            .with_header("x-amz-request-id", "123")
            .with_header("ETag", "\"abc\"");
        let resp = respond(object, &IMMUTABLE);

        assert_eq!(resp.get_status(), StatusCode::OK);
        assert_eq!(
            resp.get_header_str("Cache-Control"),
            Some("public, max-age=31536000, immutable")
        );
        assert_eq!(resp.get_header_str("ETag"), Some("\"abc\""));
        assert_eq!(resp.get_header_str("x-amz-request-id"), None);
    }

    #[test]
    fn verify_response_body_size() {
        let whole =
            Response::from_status(StatusCode::OK).with_header(header::CONTENT_LENGTH, "16424871");
        assert_eq!(body_size(&whole), Some(16_424_871));

        let partial = Response::from_status(StatusCode::PARTIAL_CONTENT)
            .with_header(header::CONTENT_RANGE, "bytes 0-99/16424871");
        assert_eq!(body_size(&partial), Some(16_424_871));

        // A cached 404, which Fastly hands to a range request as if it were the object.
        let poisoned = Response::from_status(StatusCode::PARTIAL_CONTENT)
            .with_header(header::CONTENT_RANGE, "bytes 0-99/228");
        assert_ne!(body_size(&poisoned), Some(16_424_871));

        assert_eq!(body_size(&Response::from_status(StatusCode::OK)), None);
    }

    #[test]
    fn a_missing_object_is_a_404() {
        let resp = respond(Response::from_status(StatusCode::NOT_FOUND), &SHORT);
        assert_eq!(resp.get_status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn a_bucket_error_is_a_503_to_retry() {
        let resp = respond(
            Response::from_status(StatusCode::INTERNAL_SERVER_ERROR),
            &SHORT,
        );
        assert_eq!(resp.get_status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(resp.get_header_str("Retry-After"), Some("30"));
    }
}
