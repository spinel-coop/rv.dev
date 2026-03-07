//! rv.dev — Website and installer redirect service for rv.

use fastly::convert::ToHeaderValue;
use fastly::http::{header, Method, StatusCode};
use fastly::{Error, Request, Response};

/// Create a 302 Found redirect response.
///
/// The Fastly default `Response::redirect` uses 308 Permanent Redirect, which is not
/// supported by older PowerShell versions. 302 is universally supported and
/// appropriate here since the redirect targets (e.g. `/releases/latest/...`)
/// can change over time.
fn redirect_found(destination: impl ToHeaderValue) -> Response {
    Response::from_status(StatusCode::FOUND).with_header(header::LOCATION, destination)
}

#[fastly::main]
fn main(req: Request) -> Result<Response, Error> {
    handler(req)
}

fn handler(mut req: Request) -> Result<Response, Error> {
    let service_version = std::env::var("FASTLY_SERVICE_VERSION").unwrap_or_default();

    // Remove the query string to improve cache hit ratio.
    req.remove_query();

    let path = req.get_path();
    if path == "/service-version" {
        let service_version_res =
            Response::from_body(service_version).with_content_type(fastly::mime::TEXT_PLAIN);
        return Ok(service_version_res);
    }

    // Filter request methods...
    match req.get_method() {
        // Block requests with unexpected methods
        &Method::POST | &Method::PUT | &Method::PATCH | &Method::DELETE => {
            return Ok(Response::from_status(StatusCode::METHOD_NOT_ALLOWED)
                .with_header(header::ALLOW, "GET, HEAD, PURGE")
                .with_body_text_plain("This method is not allowed\n"))
        }

        // Let any other requests through
        _ => (),
    };

    // Pattern match on the path...
    match req.get_path() {
        "/" => Ok(redirect_found("https://github.com/spinel-coop/rv/")),
        "/ruby" => Ok(redirect_found("https://github.com/spinel-coop/rv-ruby/")),
        "/ruby-dev" => Ok(redirect_found(
            "https://github.com/spinel-coop/rv-ruby-dev/",
        )),
        "/install" => {
            if req
                .get_header_str("User-Agent")
                .is_some_and(|h| h.starts_with("curl"))
            {
                Ok(redirect_found(
                    "https://github.com/spinel-coop/rv/releases/latest/download/rv-installer.sh",
                ))
            } else {
                Ok(redirect_found(
                    "https://github.com/spinel-coop/rv/releases/latest",
                ))
            }
        }
        "/install.sh" => Ok(redirect_found(
            "https://github.com/spinel-coop/rv/releases/latest/download/rv-installer.sh",
        )),
        "/install.ps1" => Ok(redirect_found(
            "https://github.com/spinel-coop/rv/releases/latest/download/rv-installer.ps1",
        )),

        // Catch all other requests and return a 404.
        _ => Ok(Response::from_status(StatusCode::NOT_FOUND)
            .with_body_text_plain("The page you requested could not be found.\n")),
    }
}
