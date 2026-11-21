//! AWS Signature Version 4, for requests without a body to an S3-compatible bucket.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

const SIGNED_HEADERS: &str = "host;x-amz-content-sha256;x-amz-date";

pub struct Credentials {
    pub access_key_id: String,
    pub secret_access_key: String,
}

/// What a request is for. `path` and `query` must already be encoded the way they are sent.
pub struct Target<'a> {
    pub method: &'a str,
    pub host: &'a str,
    pub path: &'a str,
    pub query: &'a str,
    pub region: &'a str,
}

/// The value of the `x-amz-content-sha256` header: the hash of an empty body.
pub fn content_sha256() -> String {
    hex(&Sha256::digest(b""))
}

/// The value of the `Authorization` header. `amz_date` is the request's `x-amz-date` header,
/// like `20130524T000000Z`.
pub fn authorization(credentials: &Credentials, target: &Target, amz_date: &str) -> String {
    let date = &amz_date[..8];
    let scope = format!("{date}/{}/s3/aws4_request", target.region);
    let content_sha256 = content_sha256();

    let canonical_request = format!(
        "{}\n{}\n{}\nhost:{}\nx-amz-content-sha256:{content_sha256}\nx-amz-date:{amz_date}\n\n{SIGNED_HEADERS}\n{content_sha256}",
        target.method, target.path, target.query, target.host,
    );
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex(&Sha256::digest(canonical_request.as_bytes()))
    );

    let mut key = format!("AWS4{}", credentials.secret_access_key).into_bytes();
    for part in [date, target.region, "s3", "aws4_request"] {
        key = hmac(&key, part);
    }
    let signature = hex(&hmac(&key, &string_to_sign));

    format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={SIGNED_HEADERS}, Signature={signature}",
        credentials.access_key_id
    )
}

fn hmac(key: &[u8], message: &str) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts a key of any length");
    mac.update(message.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The examples in Amazon's "Authenticating Requests: Using the Authorization Header" guide.
    fn example(query: &str) -> String {
        let credentials = Credentials {
            access_key_id: "AKIAIOSFODNN7EXAMPLE".to_string(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
        };
        let target = Target {
            method: "GET",
            host: "examplebucket.s3.amazonaws.com",
            path: "/",
            query,
            region: "us-east-1",
        };
        authorization(&credentials, &target, "20130524T000000Z")
    }

    #[test]
    fn signs_like_the_get_bucket_lifecycle_example() {
        assert_eq!(
            example("lifecycle="),
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, \
             SignedHeaders=host;x-amz-content-sha256;x-amz-date, \
             Signature=fea454ca298b7da1c68078a5d1bdbfbbe0d65c699e0f91ac7a200a0136783543"
        );
    }

    #[test]
    fn signs_like_the_list_objects_example() {
        assert!(example("max-keys=2&prefix=J").ends_with(
            "Signature=34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7"
        ));
    }

    #[test]
    fn empty_body_hash() {
        assert_eq!(
            content_sha256(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
