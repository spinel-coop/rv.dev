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
        "/ai-policy" => Ok(redirect_found(
            "https://github.com/spinel-coop/rv/blob/main/AI_POLICY.md",
        )),

        // Catch all other requests and return a 404.
        _ => Ok(Response::from_status(StatusCode::NOT_FOUND)
            .with_body_text_plain("The page you requested could not be found.\n")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_redirect(resp: &Response, expected_location: &str) {
        assert_eq!(resp.get_status(), StatusCode::FOUND);
        assert_eq!(resp.get_header_str("Location"), Some(expected_location),);
    }

    #[test]
    fn root_redirects_to_github() {
        let req = Request::get("http://rv.dev/");
        let resp = handler(req).expect("request succeeds");
        assert_redirect(&resp, "https://github.com/spinel-coop/rv/");
    }

    #[test]
    fn ruby_redirects() {
        let req = Request::get("http://rv.dev/ruby");
        let resp = handler(req).expect("request succeeds");
        assert_redirect(&resp, "https://github.com/spinel-coop/rv-ruby/");
    }

    #[test]
    fn ruby_dev_redirects() {
        let req = Request::get("http://rv.dev/ruby-dev");
        let resp = handler(req).expect("request succeeds");
        assert_redirect(&resp, "https://github.com/spinel-coop/rv-ruby-dev/");
    }

    #[test]
    fn install_with_curl_redirects_to_sh() {
        let mut req = Request::get("http://rv.dev/install");
        req.set_header("User-Agent", "curl/8.0");
        let resp = handler(req).expect("request succeeds");
        assert_redirect(
            &resp,
            "https://github.com/spinel-coop/rv/releases/latest/download/rv-installer.sh",
        );
    }

    #[test]
    fn install_without_curl_redirects_to_releases() {
        let req = Request::get("http://rv.dev/install");
        let resp = handler(req).expect("request succeeds");
        assert_redirect(&resp, "https://github.com/spinel-coop/rv/releases/latest");
    }

    #[test]
    fn install_sh_redirects() {
        let req = Request::get("http://rv.dev/install.sh");
        let resp = handler(req).expect("request succeeds");
        assert_redirect(
            &resp,
            "https://github.com/spinel-coop/rv/releases/latest/download/rv-installer.sh",
        );
    }

    #[test]
    fn install_ps1_redirects() {
        let req = Request::get("http://rv.dev/install.ps1");
        let resp = handler(req).expect("request succeeds");
        assert_redirect(
            &resp,
            "https://github.com/spinel-coop/rv/releases/latest/download/rv-installer.ps1",
        );
    }

    #[test]
    fn unknown_path_returns_404() {
        let req = Request::get("http://rv.dev/nonexistent");
        let resp = handler(req).expect("request succeeds");
        assert_eq!(resp.get_status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn post_returns_405() {
        let req = Request::post("http://rv.dev/");
        let resp = handler(req).expect("request succeeds");
        assert_eq!(resp.get_status(), StatusCode::METHOD_NOT_ALLOWED);
    }
}
