//! Default Compute template program.

use fastly::http::{header, Method, StatusCode};
use fastly::{Error, Request, Response};

#[fastly::main]
fn main(mut req: Request) -> Result<Response, Error> {
    // let local_dev = std::env::var("FASTLY_HOSTNAME").unwrap_or_default() == "localhost";
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
        // If request is to the `/` path...
        "/" => Ok(Response::redirect("https://github.com/spinel-coop/rv/")),
        "/ruby" => Ok(Response::redirect(
            "https://github.com/spinel-coop/rv-ruby/",
        )),
        "/ruby-dev" => Ok(Response::redirect(
            "https://github.com/spinel-coop/rv-ruby-dev/",
        )),
        "/install" | "/install.sh" => Ok(Response::redirect(
            "https://github.com/spinel-coop/rv/releases/latest/download/rv-installer.sh",
        )),
        "/install.ps1" => Ok(Response::redirect(
            "https://github.com/spinel-coop/rv/releases/latest/download/rv-installer.ps1",
        )),

        // Catch all other requests and return a 404.
        _ => Ok(Response::from_status(StatusCode::NOT_FOUND)
            .with_body_text_plain("The page you requested could not be found.\n")),
    }
}
