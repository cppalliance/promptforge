//! AWS Signature Version 4 signing for the Bedrock list request: a
//! hand-assembled HMAC-SHA256 `Authorization` header over the
//! canonical GET. The signer sits in this sibling module so the
//! provider file stays under the workspace's 500-line ceiling.

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

/// The SigV4 basic-format amz date; the signer re-derives the date
/// stamp from its first eight characters.
pub(super) fn amz_date(now: OffsetDateTime) -> String {
    let date = now.date();
    let time = now.time();
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        date.year(),
        u8::from(date.month()),
        date.day(),
        time.hour(),
        time.minute(),
        time.second()
    )
}

/// The host (with any non-default port) named by `url`.
pub(super) fn host_of(url: &str) -> &str {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    after_scheme.split('/').next().unwrap_or(after_scheme)
}

/// Signs a GET request per AWS Signature Version 4, returning the
/// `Authorization` header value. `query` is the canonical query string
/// (name-sorted, URI-encoded); the Bedrock list endpoint takes none.
#[expect(
    clippy::too_many_arguments,
    reason = "the eight inputs are the SigV4 canonical request fields; a struct would restate them once more"
)]
pub(super) fn sign_get(
    host: &str,
    path: &str,
    query: &str,
    region: &str,
    service: &str,
    access_key: &str,
    secret_key: &str,
    amz_date: &str,
) -> String {
    let payload_hash = hex_lower(&Sha256::digest(b""));
    let canonical_request = format!(
        "GET\n{path}\n{query}\nhost:{host}\nx-amz-date:{amz_date}\n\nhost;x-amz-date\n{payload_hash}"
    );
    let date_stamp = &amz_date[..8];
    let scope = format!("{date_stamp}/{region}/{service}/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex_lower(&Sha256::digest(canonical_request.as_bytes()))
    );
    let k_date = hmac_sha256(
        format!("AWS4{secret_key}").as_bytes(),
        date_stamp.as_bytes(),
    );
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    let k_signing = hmac_sha256(&k_service, b"aws4_request");
    let signature = hex_lower(&hmac_sha256(&k_signing, string_to_sign.as_bytes()));
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key}/{scope}, SignedHeaders=host;x-amz-date, Signature={signature}"
    )
}

/// HMAC-SHA256 over `data` under `key`.
fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(key)
        .unwrap_or_else(|_| unreachable!("HMAC-SHA256 accepts a key of any length"));
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// Lowercase hex, as SigV4 renders hashes and signatures.
fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::sign_get;

    /// Signs with the AWS SigV4 test-suite credentials, host, and date.
    fn sign_vector(path: &str, query: &str) -> String {
        sign_get(
            "example.amazonaws.com",
            path,
            query,
            "us-east-1",
            "service",
            "AKIDEXAMPLE",
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "20150830T123600Z",
        )
    }

    /// AWS's published SigV4 test-suite vector `get-vanilla`: an empty
    /// path-less GET against the example host.
    #[test]
    fn sigv4_matches_aws_get_vanilla_vector() {
        assert_eq!(
            sign_vector("/", ""),
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
             SignedHeaders=host;x-amz-date, \
             Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
    }

    /// AWS's published SigV4 test-suite vector
    /// `get-vanilla-query-order-key-case`: the canonical query string is
    /// name-sorted regardless of wire order.
    #[test]
    fn sigv4_matches_aws_sorted_query_vector() {
        assert_eq!(
            sign_vector("/", "Param1=value1&Param2=value2"),
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
             SignedHeaders=host;x-amz-date, \
             Signature=b97d918cfa904a5beff61c982a1b6f458b799221646efd99d3219ec94cdf2500"
        );
    }
}
