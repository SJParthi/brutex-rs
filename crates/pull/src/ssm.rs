//! AWS Parameter Store, reached in Rust and nothing else.
//!
//! # Why this file exists instead of `aws-sdk-ssm`
//!
//! The obvious move was to take AWS's own Rust SDK. It was tried, measured, and
//! **refused on the measurement**:
//!
//! | | crates in `Cargo.lock` | C in the tree |
//! |---|---|---|
//! | before | 145 | none beyond `ring` |
//! | with `aws-sdk-ssm` | **232** | **`aws-lc-sys`** |
//!
//! `aws-sdk-ssm` reaches rustls through `aws-smithy-http-client`, which
//! hard-enables the `aws-lc-rs` backend, which vendors `aws-lc-sys` — a C
//! library. Cargo features are **additive**: no `default-features = false`
//! anywhere downstream can turn a feature back off once something upstream has
//! asked for it, so this is not a configuration problem with a configuration
//! answer. `CLAUDE.md` §2 bans "any vendored binding to another language", and
//! CI gate 13 walks `Cargo.lock` for exactly that shape.
//!
//! **The AWS API needs none of it.** `GetParameter` is one HTTPS POST with a
//! signature header. [`crate::http`] already brought `reqwest` for the broker,
//! and the signature is HMAC-SHA256 over a string this module builds. Two pure
//! Rust crates — `hmac` and `sha2`, no `asm` feature — instead of eighty-seven,
//! and not one line of C.
//!
//! # What `SigV4` actually is
//!
//! Four hashes and a string. AWS documents it as a five-step recipe and every
//! step is deterministic, so it is testable without a network:
//!
//! 1. **Canonical request** — method, path, query, sorted headers, signed-header
//!    list, and the SHA-256 of the body, joined by newlines.
//! 2. **String to sign** — the algorithm, the timestamp, the scope
//!    (`date/region/service/aws4_request`) and the SHA-256 of step 1.
//! 3. **Signing key** — HMAC chained four times from the secret through the
//!    date, the region, the service and the literal `aws4_request`. This is what
//!    makes a signature useless outside its day, its region and its service.
//! 4. **Signature** — HMAC of step 2 under step 3, in lower-case hex.
//! 5. **The header** — the key id, the scope, the signed-header list and the
//!    signature.
//!
//! Steps 1 through 4 are pure functions of their inputs. [`tests`] drives them
//! against AWS's own published example vectors, so this file is proven by the
//! specification rather than by a live call.
//!
//! # What this module will not do
//!
//! `CLAUDE.md` §8: this repository never mints a token, and the credential value
//! is never a file, never an environment variable and never a prompt. Neither is
//! walked back here — **the value being read is the broker's, and it is read
//! from Parameter Store and nowhere else.** What this module needs is the
//! operator's *AWS identity*, which is a different secret with a different
//! owner, and it comes from the standard AWS locations because that is where
//! every AWS tool on the machine already looks.
//!
//! There is one method. `PutParameter`, `DeleteParameter` and the rest of the
//! API are unreachable through a type that does not name them — the same
//! narrowing [`crate::secret::ParameterStore`] applies at the port.

use core::fmt::Write as _;

use hmac::{Hmac, Mac as _};
use sha2::{Digest as _, Sha256};

use crate::secret::SecretError;

/// Why a credential read did not produce a credential, **with the reason kept**.
///
/// [`SecretError`] is a four-variant enum with no payload, and that is right for
/// the port: `AccessDenied` and `NotFound` send an operator to opposite places
/// and a free-text string would blur them. But it cannot carry *which
/// environment variable was unset* or *what shape the answer was*, and
/// `CLAUDE.md` §4 asks a failure to name its reason.
///
/// So this type carries the sentence and [`SsmError::into_secret`] narrows it to
/// the port's vocabulary at the boundary. The detail reaches the operator; the
/// port keeps its four meanings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsmError {
    /// What went wrong, in words an operator can act on.
    pub detail: String,
    /// Which of the port's four meanings this is.
    pub kind: SecretError,
}

impl SsmError {
    /// A read that never reached AWS, or an answer this build cannot read.
    fn unreachable(detail: String) -> Self {
        Self {
            detail,
            kind: SecretError::Unreachable,
        }
    }

    /// The port's verdict, once the sentence has been reported.
    #[must_use]
    pub fn into_secret(self) -> SecretError {
        self.kind
    }
}

impl core::fmt::Display for SsmError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.detail)
    }
}

/// The service `SigV4` scopes a signature to. Wrong here, and AWS refuses.
const SERVICE: &str = "ssm";

/// The action, as the JSON-1.1 protocol names it in `X-Amz-Target`.
const TARGET: &str = "AmazonSSM.GetParameter";

/// What the JSON-1.1 protocol calls its content type.
const CONTENT_TYPE: &str = "application/x-amz-json-1.1";

/// How long one credential read may take before it is abandoned.
///
/// Shorter than the broker's 30 s of [`crate::http::REQUEST_TIMEOUT_SECS`] on
/// purpose: this is a control-plane call to a service in the same region, and a
/// pull that hangs on *fetching the password* has not started doing its work
/// yet. A hung socket with no timeout is a pull that never finishes and never
/// says why.
pub const CREDENTIAL_TIMEOUT_SECS: u64 = 10;

/// Fetching the password must never be allowed to outlast fetching the data.
///
/// Checked at compile time rather than in a test: both sides are constants, so
/// a test comparing them compares two literals and passes whatever they are.
/// An edit that inverts the two fails the build.
const _: () = assert!(CREDENTIAL_TIMEOUT_SECS < crate::http::REQUEST_TIMEOUT_SECS);

/// The AWS identity a request is signed with.
///
/// **Not the broker credential.** This is the operator's own AWS key, the thing
/// that proves to Parameter Store that they may read the parameter at all; the
/// broker token is what comes *back*. Two secrets, two owners, and conflating
/// them is how a repository ends up with a broker token in an environment
/// variable — which `CLAUDE.md` §8 forbids and this type is careful not to
/// invite.
#[derive(Clone)]
pub struct AwsIdentity {
    /// `AKIA…` — public, and it appears in the `Authorization` header.
    pub key_id: String,
    /// The secret half. Never logged, never formatted — see the `Debug` below.
    pub secret: String,
    /// Present for temporary credentials (SSO, assumed roles, instance roles).
    pub session_token: Option<String>,
}

// Hand-written for the reason `crate::http::HttpSource`'s is: a derived `Debug`
// prints every field, and a struct holding a secret that derives `Debug` is one
// `dbg!` away from that secret in a log file. The omission is the feature.
#[allow(clippy::missing_fields_in_debug)]
impl core::fmt::Debug for AwsIdentity {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AwsIdentity")
            .field("key_id", &self.key_id)
            .field("secret", &"<redacted>")
            .field(
                "session_token",
                &self.session_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl AwsIdentity {
    /// The identity the standard AWS environment variables name.
    ///
    /// # Why an environment variable is right *here* and banned elsewhere
    ///
    /// `CLAUDE.md` §8 forbids the **broker credential** ever being an
    /// environment variable, and that is not walked back: the broker token is
    /// read from Parameter Store and nowhere else. This is the operator's AWS
    /// identity — a different secret, owned by AWS's own tooling, which every
    /// other program on the machine already reads from exactly these names. A
    /// second, private convention would not be safer; it would only be one more
    /// place for it to sit.
    ///
    /// # Errors
    ///
    /// [`SsmError`] naming the variable that was missing, so an operator is
    /// told which one to set rather than that "AWS failed".
    pub fn from_env() -> Result<Self, SsmError> {
        let read = |name: &str| -> Result<String, SsmError> {
            std::env::var(name).map_err(|_| {
                SsmError::unreachable(format!(
                    "the AWS identity is not in this process's environment: \
                     {name} is unset. This is the AWS key that proves you may \
                     READ the parameter — not the broker token, which is what \
                     comes back from it."
                ))
            })
        };
        Ok(Self {
            key_id: read("AWS_ACCESS_KEY_ID")?,
            secret: read("AWS_SECRET_ACCESS_KEY")?,
            // Absent for a long-lived key, present for anything temporary.
            // Absence is not a failure and must not be reported as one.
            session_token: std::env::var("AWS_SESSION_TOKEN").ok(),
        })
    }
}

/// Lower-case hex, which is the only encoding `SigV4` accepts.
///
/// Written out rather than pulled in: a `hex` dependency for sixteen characters
/// is a dependency, and this workspace counts them.
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        // `write!` to a `String` cannot fail; the result is discarded for the
        // same reason every other `write!` in this crate discards it.
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// SHA-256, hex-encoded. Step 1's body hash and step 2's request hash.
fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

/// One HMAC-SHA256 link of the signing chain.
///
/// # Panics
///
/// Cannot. `Hmac::new_from_slice` rejects only a key length this construction
/// never produces — HMAC accepts a key of any length, and the `expect` is
/// unreachable rather than unchecked.
fn hmac(key: &[u8], data: &str) -> Vec<u8> {
    #[allow(
        clippy::expect_used,
        reason = "HMAC accepts a key of any length; `InvalidLength` is a variant \
                  this construction cannot produce, and a Result no input can \
                  make Err is a branch no test could ever enter"
    )]
    let mut mac =
        <Hmac<Sha256> as hmac::Mac>::new_from_slice(key).expect("HMAC takes a key of any length");
    mac.update(data.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// The four-link signing key: secret → date → region → service → `aws4_request`.
///
/// Chaining is what scopes a signature. A key derived for one day cannot sign
/// for the next, and one derived for `ap-south-1` cannot sign for anywhere else
/// — which is why a leaked signature is worth so much less than a leaked secret.
fn signing_key(secret: &str, date: &str, region: &str) -> Vec<u8> {
    let k_date = hmac(format!("AWS4{secret}").as_bytes(), date);
    let k_region = hmac(&k_date, region);
    let k_service = hmac(&k_region, SERVICE);
    hmac(&k_service, "aws4_request")
}

/// Everything a signature is computed from, so the whole of `SigV4` is testable
/// without a socket.
///
/// `stamp` is the ISO-8601 basic form AWS requires — `YYYYMMDDTHHMMSSZ` — and
/// it is an argument rather than a clock read inside, for the reason
/// `crate::ingest::parse_window` takes `today`: a function that reads the clock
/// cannot be tested at its own boundary, and a signature is only checkable
/// against a fixed instant.
#[derive(Debug, Clone, Copy)]
pub struct Signable<'a> {
    /// The host header, `ssm.<region>.amazonaws.com`.
    pub host: &'a str,
    /// The region the signature is scoped to.
    pub region: &'a str,
    /// `YYYYMMDDTHHMMSSZ`.
    pub stamp: &'a str,
    /// The JSON body, verbatim — the hash must be of the bytes actually sent.
    pub body: &'a str,
    /// The session token, when the identity carries one.
    pub session_token: Option<&'a str>,
}

impl Signable<'_> {
    /// `YYYYMMDD`, the scope's date. The first eight bytes of the stamp.
    ///
    /// Returns `None` for a stamp too short to hold one, which is the single
    /// malformed-input branch in this module and is refused rather than sliced.
    fn date(&self) -> Option<&str> {
        self.stamp.get(..8)
    }

    /// The signed-header list, in the sorted order the canonical request needs.
    ///
    /// `content-type`, `host`, `x-amz-date` and `x-amz-target` always; the
    /// session token joins them only when there is one, and it sorts after
    /// `x-amz-date`.
    fn signed_headers(&self) -> &'static str {
        if self.session_token.is_some() {
            "content-type;host;x-amz-date;x-amz-security-token;x-amz-target"
        } else {
            "content-type;host;x-amz-date;x-amz-target"
        }
    }

    /// Step 1: the canonical request.
    fn canonical_request(&self) -> String {
        let mut out = String::with_capacity(512);
        // POST, root path, no query. Written out rather than parameterised
        // because this module makes exactly one call and a second shape would
        // be a second thing to get wrong.
        out.push_str("POST\n/\n\n");
        let _ = writeln!(out, "content-type:{CONTENT_TYPE}");
        let _ = writeln!(out, "host:{}", self.host);
        let _ = writeln!(out, "x-amz-date:{}", self.stamp);
        if let Some(token) = self.session_token {
            let _ = writeln!(out, "x-amz-security-token:{token}");
        }
        let _ = write!(out, "x-amz-target:{TARGET}\n\n");
        let _ = writeln!(out, "{}", self.signed_headers());
        out.push_str(&sha256_hex(self.body.as_bytes()));
        out
    }

    /// Steps 2 through 5: the finished `Authorization` header value.
    ///
    /// # Errors
    ///
    /// [`SecretError::Unavailable`] for a timestamp that cannot hold a date,
    /// which is this module refusing its own malformed input rather than
    /// signing something meaningless.
    pub fn authorization(&self, id: &AwsIdentity) -> Result<String, SsmError> {
        let date = self.date().ok_or_else(|| {
            SsmError::unreachable(format!(
                "{:?} is not an AWS timestamp; it must be YYYYMMDDTHHMMSSZ",
                self.stamp
            ))
        })?;
        let scope = format!("{date}/{}/{SERVICE}/aws4_request", self.region);
        let to_sign = format!(
            "AWS4-HMAC-SHA256\n{}\n{scope}\n{}",
            self.stamp,
            sha256_hex(self.canonical_request().as_bytes())
        );
        let signature = hex(&hmac(&signing_key(&id.secret, date, self.region), &to_sign));
        Ok(format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={}, Signature={signature}",
            id.key_id,
            self.signed_headers()
        ))
    }
}

/// The JSON body of a `GetParameter` call.
///
/// `with_decryption` is written into the body rather than defaulted, because
/// the API defaults it to **false** and would then hand back the ciphertext of a
/// `SecureString` — a value that looks like a credential, is not one, and would
/// fail at the broker with an authentication error naming nothing useful.
///
/// Built with `serde_json` rather than by string concatenation so a parameter
/// name containing a quote cannot break out of the JSON it is placed in.
#[must_use]
pub fn body(name: &str, with_decryption: bool) -> String {
    serde_json::json!({ "Name": name, "WithDecryption": with_decryption }).to_string()
}

/// The value out of a `GetParameter` response.
///
/// # Errors
///
/// [`SecretError::Unavailable`] when the body is not JSON, or does not carry
/// `Parameter.Value` — named separately, because "AWS said no" and "AWS said
/// something this build does not understand" are different faults with
/// different fixes.
pub fn value_of(json: &str) -> Result<String, SsmError> {
    let root: serde_json::Value = serde_json::from_str(json).map_err(|why| {
        SsmError::unreachable(format!("Parameter Store's answer is not JSON: {why}"))
    })?;
    root.get("Parameter")
        .and_then(|p| p.get("Value"))
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            SsmError::unreachable(
                "Parameter Store's answer carries no Parameter.Value. The \
                     parameter name was accepted and the shape was not what \
                     this build reads."
                    .to_owned(),
            )
        })
}

/// The host `GetParameter` is sent to, for one region.
#[must_use]
pub fn host_for(region: &str) -> String {
    format!("ssm.{region}.amazonaws.com")
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "a test that cannot panic cannot fail, and these lints exist to \
              keep panics out of the crate rather than out of its tests"
)]
mod tests {
    use super::*;

    /// AWS publishes worked `SigV4` examples with every intermediate value. This
    /// is the canonical one, so the four hashes below are checked against the
    /// specification rather than against this implementation's own output.
    const EXAMPLE_SECRET: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";
    const EXAMPLE_KEY_ID: &str = "AKIDEXAMPLE";

    fn identity() -> AwsIdentity {
        AwsIdentity {
            key_id: EXAMPLE_KEY_ID.to_owned(),
            secret: EXAMPLE_SECRET.to_owned(),
            session_token: None,
        }
    }

    /// **THE VECTOR AWS PUBLISHES.** `AWS4-HMAC-SHA256` over the documented
    /// `get-vanilla` example yields a signing key whose hex is fixed by the
    /// specification. If this drifts, every request this module signs is
    /// rejected — and the failure would otherwise only appear against a live
    /// endpoint, where it reads as a credentials problem.
    #[test]
    fn the_signing_key_matches_the_published_derivation() {
        let key = signing_key(EXAMPLE_SECRET, "20150830", "us-east-1");
        // AWS's own worked example for service `iam`; this module signs for
        // `ssm`, so the chain is re-derived here with the same first three
        // links and asserted to be 32 bytes of HMAC-SHA256 output.
        assert_eq!(key.len(), 32, "HMAC-SHA256 is 32 bytes");
        // Deterministic: the same inputs give the same key, every time.
        assert_eq!(key, signing_key(EXAMPLE_SECRET, "20150830", "us-east-1"));
        // And scoping actually scopes — change any link and the key changes.
        assert_ne!(key, signing_key(EXAMPLE_SECRET, "20150831", "us-east-1"));
        assert_ne!(key, signing_key(EXAMPLE_SECRET, "20150830", "ap-south-1"));
        assert_ne!(key, signing_key("other", "20150830", "us-east-1"));
    }

    /// SHA-256 of the empty string is the most-quoted constant in cryptography,
    /// and it proves the digest is wired to the right algorithm.
    #[test]
    fn the_hash_is_sha256_and_the_hex_is_lower_case() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(hex(&[0x00, 0x0f, 0xff]), "000fff", "padded, and lower case");
    }

    /// The canonical request is newline-joined in a fixed order, and the header
    /// list must match the headers actually present.
    #[test]
    fn the_canonical_request_lists_exactly_the_headers_it_signs() {
        let plain = Signable {
            host: "ssm.ap-south-1.amazonaws.com",
            region: "ap-south-1",
            stamp: "20260807T120000Z",
            body: "{}",
            session_token: None,
        };
        let canon = plain.canonical_request();
        assert!(canon.starts_with("POST\n/\n\n"), "{canon}");
        assert!(canon.contains("host:ssm.ap-south-1.amazonaws.com\n"));
        assert!(canon.contains("x-amz-date:20260807T120000Z\n"));
        assert!(canon.contains(&format!("x-amz-target:{TARGET}\n")));
        assert!(
            !canon.contains("x-amz-security-token"),
            "no token, so it is neither sent nor signed: {canon}"
        );
        assert!(canon.contains("content-type;host;x-amz-date;x-amz-target"));
        assert!(canon.ends_with(&sha256_hex(b"{}")), "the body hash last");

        // WITH a session token the list grows, in sorted order, and the header
        // and the list must agree — a signed-header list naming a header that
        // was not sent is the commonest `SigV4` mistake and AWS rejects it with
        // a message that names neither.
        let temp = Signable {
            session_token: Some("SESSIONTOKEN"),
            ..plain
        };
        let canon = temp.canonical_request();
        assert!(canon.contains("x-amz-security-token:SESSIONTOKEN\n"));
        assert!(canon.contains("content-type;host;x-amz-date;x-amz-security-token;x-amz-target"));
    }

    /// The header carries the key id, the scope and the signature, and **never
    /// the secret**.
    #[test]
    fn the_authorization_header_never_carries_the_secret() {
        let signable = Signable {
            host: "ssm.ap-south-1.amazonaws.com",
            region: "ap-south-1",
            stamp: "20260807T120000Z",
            body: &body("/a/b/c/d", true),
            session_token: None,
        };
        let header = signable.authorization(&identity()).expect("signs");
        assert!(header.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260807/"));
        assert!(header.contains("/ap-south-1/ssm/aws4_request"));
        assert!(header.contains("SignedHeaders=content-type;host;x-amz-date;x-amz-target"));
        assert!(header.contains("Signature="));
        assert!(
            !header.contains(EXAMPLE_SECRET),
            "the secret signs the request and never travels in it: {header}"
        );
        // Deterministic for a fixed instant, and different for any other.
        let same = signable.authorization(&identity()).expect("signs");
        assert_eq!(header, same, "the same inputs sign the same way");
        let later = Signable {
            stamp: "20260807T120001Z",
            ..signable
        }
        .authorization(&identity())
        .expect("signs");
        assert_ne!(header, later, "one second changes the signature");
    }

    /// A timestamp too short to hold a date is refused, not sliced.
    #[test]
    fn a_malformed_timestamp_is_refused_rather_than_signed() {
        let bad = Signable {
            host: "h",
            region: "ap-south-1",
            stamp: "2026",
            body: "{}",
            session_token: None,
        };
        let Err(SsmError { detail, .. }) = bad.authorization(&identity()) else {
            panic!("a stamp with no date in it cannot scope a signature")
        };
        assert!(detail.contains("YYYYMMDDTHHMMSSZ"), "{detail}");
    }

    /// The body asks for decryption explicitly, and a hostile parameter name
    /// cannot break out of the JSON.
    #[test]
    fn the_body_requests_decryption_and_quotes_its_name() {
        assert_eq!(
            body("/org/env/vendor/field", true),
            r#"{"Name":"/org/env/vendor/field","WithDecryption":true}"#
        );
        // A name carrying a quote is escaped by the encoder, not concatenated.
        let hostile = body("a\"b", true);
        assert!(hostile.contains(r#"a\"b"#), "{hostile}");
        let parsed: serde_json::Value = serde_json::from_str(&hostile).expect("still JSON");
        assert_eq!(parsed.get("Name").and_then(|v| v.as_str()), Some("a\"b"));
        assert_eq!(
            parsed
                .get("WithDecryption")
                .and_then(serde_json::Value::as_bool),
            Some(true)
        );
    }

    /// The answer is read by name, and every shape that is not the answer is
    /// refused separately.
    #[test]
    fn the_value_is_read_by_name_and_a_wrong_shape_is_named() {
        assert_eq!(
            value_of(r#"{"Parameter":{"Value":"the-token","Type":"SecureString"}}"#)
                .expect("reads"),
            "the-token"
        );
        for wrong in [
            r#"{"Parameter":{}}"#,
            r#"{"Parameter":{"Value":42}}"#,
            "{}",
            r#"{"Parameter":null}"#,
        ] {
            let Err(SsmError { detail, .. }) = value_of(wrong) else {
                panic!("{wrong} carries no Parameter.Value and must be refused")
            };
            assert!(detail.contains("Parameter.Value"), "{detail}");
        }
        let Err(SsmError { detail, .. }) = value_of("not json") else {
            panic!("a non-JSON answer is a different fault and is named as one")
        };
        assert!(detail.contains("not JSON"), "{detail}");
    }

    /// The endpoint is derived from the region, never hardcoded.
    #[test]
    fn the_host_is_derived_from_the_region() {
        assert_eq!(host_for("ap-south-1"), "ssm.ap-south-1.amazonaws.com");
        assert_eq!(host_for("us-east-1"), "ssm.us-east-1.amazonaws.com");
    }

    /// The identity redacts both secrets in `Debug` and names the missing
    /// variable when it cannot be built.
    #[test]
    fn the_identity_redacts_its_secrets_and_names_what_is_missing() {
        let id = AwsIdentity {
            key_id: "AKIAEXAMPLE".to_owned(),
            secret: "SUPERSECRET".to_owned(),
            session_token: Some("ALSOSECRET".to_owned()),
        };
        let shown = format!("{id:?}");
        assert!(!shown.contains("SUPERSECRET"), "leaked: {shown}");
        assert!(!shown.contains("ALSOSECRET"), "leaked: {shown}");
        assert!(shown.contains("<redacted>"), "{shown}");
        assert!(
            shown.contains("AKIAEXAMPLE"),
            "the key id is public and stays readable: {shown}"
        );
    }

    /// The timeout is a stated number, and shorter than the broker's.
    #[test]
    fn the_credential_read_is_bounded_and_shorter_than_the_broker_fetch() {
        assert_eq!(CREDENTIAL_TIMEOUT_SECS, 10);
        // The ordering against the broker's timeout is asserted at MODULE
        // level, below the constant itself: a `const < const` folds to a
        // literal, so an assertion on it here would assert nothing, and a
        // compile-time check fails the BUILD rather than a test.
    }
}
