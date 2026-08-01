//! Reading the credential, and the two ways that stops rather than degrades.
//!
//! # The surface is one method, and it reads
//!
//! ```text
//! trait SecretSource { fn read(&self, path) -> Result<Secret, SecretError>; }
//! ```
//!
//! `CLAUDE.md` §8: *"This repository never mints a token."* There is no `write`,
//! no `put`, no `create`, no `rotate` and no `mint` on [`SecretSource`], on
//! [`ParameterStore`], or anywhere else in this crate. That is the structural
//! half of invariant P-05 — a token cannot be minted by code that does not
//! exist, and adding the method is a diff a reviewer sees rather than a call
//! buried in an SDK builder.
//!
//! The behavioural half is `pull::unit::readonly_credentials`, which drives a
//! whole credential read through a double whose write **panics**, and asserts
//! afterwards that the write was never reached. The same test calls the write
//! directly and requires it to panic, because a double that would not have
//! failed proves nothing about the code that did not call it.
//!
//! # Why a local mint would be worse than a failed pull
//!
//! `CLAUDE.md` §8 again: *"A local mint would invalidate the token another
//! system shares."* Minting is not merely out of scope — it is actively
//! destructive, and it fails in the other system rather than in this one, which
//! is the hardest shape of failure to attribute.
//!
//! # An auth failure halts (P-06)
//!
//! [`CredentialReader::read`] returns [`CredentialHalt`] and has no arm that
//! returns a value the source did not give it. There is no cached credential,
//! no retry-with-a-different-path, no anonymous mode and no "continue without
//! the vendor". `CLAUDE.md` §4: degrade loudly and name the reason, or refuse —
//! never both silently.
//!
//! # A dead token is re-read once, and then it halts
//!
//! `CLAUDE.md` §8: *"A stale token is re-read; if the re-read returns the same
//! dead value, the pull halts loudly."* [`CredentialReader::reread_after_rejection`]
//! is that sentence as a function. It takes the value the vendor just rejected,
//! reads the parameter again, and:
//!
//! | The re-read returns | What happens |
//! |---|---|
//! | a different value | the rotation landed — the new value is returned |
//! | the same value | [`CredentialHalt::DeadToken`], naming the vendor and the field |
//! | an error | that error's halt, unchanged |
//!
//! There is no third read, no backoff loop and no mint. The comparison is the
//! whole mechanism: a token that a vendor rejected and that Parameter Store
//! still holds is a token nothing in this repository can fix.
//!
//! # Nothing here ever prints a credential
//!
//! [`Secret`] has a hand-written [`std::fmt::Debug`] that renders a redaction
//! and its length, and it has **no** [`std::fmt::Display`]. Reaching the value
//! is [`Secret::expose`], which is spelled to be visible in a diff.
//! [`CredentialHalt`] carries the vendor and the field *name*; it never carries
//! the value, and it never carries the assembled path, which names an account.
//! [`crate::config::CredentialPath`] and [`crate::config::CredentialConfig`]
//! carry hand-written `Debug` implementations for the same reason — until
//! D-0036 they carried derives, so the path this type was careful not to hold
//! was printable from the type that does hold it. P-11 and P-18.
//!
//! # What this port cannot tell, and says so
//!
//! [`SsmSecretSource::read`] always passes `with_decryption: true`, and it has
//! **no** check that what came back is not ciphertext. There was a
//! `SecretError::NotDecrypted` variant here describing such a check; nothing
//! ever constructed it, so it advertised a protection that did not exist and
//! D-0036 removed it. Telling a decrypted value from a `SecureString`'s
//! ciphertext means recognising a KMS blob's shape, which is an external fact
//! with no source recorded in `docs/00-charter.md` — `CLAUDE.md` §3 rule 1
//! forbids inventing one. If it ever happens the vendor rejects the value, the
//! re-read returns the same ciphertext, and the halt reads
//! [`CredentialHalt::DeadToken`], which is loud but names the wrong cause.
//! `docs/06-limits.md` §19 records that, rather than a doc comment describing a
//! check nobody wrote.
//!
//! # There is no AWS SDK in this change
//!
//! [`ParameterStore`] is the port: one method, the two arguments the real API
//! takes, and a `Result`. [`SsmSecretSource`] is the adapter over it, and it is
//! fully exercised against a double. The change that first makes a live call
//! writes one `impl ParameterStore` over the SDK client and takes the
//! dependency then — with `cargo deny check` run against it and the result
//! stated. Taking it *now* would add about a hundred transitive crates to
//! `Cargo.lock` to support no call at all. See `docs/05-decisions.md` D-0035.

use std::fmt;

use brutex_core::vendor::Vendor;

use crate::config::CredentialPath;

/// A credential value, held so it cannot be printed by accident.
///
/// Not `Copy`, not `Display`, and its `Debug` is a redaction. `PartialEq` is
/// derived because comparing a re-read against the value a vendor rejected is
/// the entire dead-token mechanism.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wraps a value read from a secret source.
    ///
    /// # Errors
    ///
    /// [`SecretError::Empty`] for an empty value. An empty `SecureString` is a
    /// parameter that exists and holds nothing, which every downstream call
    /// would report as an auth failure against the vendor rather than as the
    /// configuration fault it is.
    pub fn new(value: String) -> Result<Self, SecretError> {
        if value.is_empty() {
            return Err(SecretError::Empty);
        }
        Ok(Self(value))
    }

    /// The value.
    ///
    /// Spelled `expose` rather than `as_str` so that every place a credential
    /// leaves this type is greppable, and obvious in review.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// How many bytes the value holds.
    ///
    /// The one property of a credential that is safe to report.
    ///
    /// Spelled `byte_len` rather than `len` deliberately. `len` obliges clippy
    /// to demand an `is_empty` beside it, and `is_empty` on this type can only
    /// ever return `false` — [`Secret::new`] refuses an empty value — which
    /// makes it a function no test can distinguish from a constant. D-0036:
    /// a method whose only possible answer is one value is not covered by a
    /// test that calls it, it is merely executed by one.
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.0.len()
    }
}

impl fmt::Debug for Secret {
    /// Renders the length and nothing else.
    ///
    /// A derived `Debug` would put the credential into any `{:?}`, any
    /// `assert_eq!` failure and any error chain that happened to contain one.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Secret(<redacted, {} bytes>)", self.0.len())
    }
}

/// Why a secret source could not produce a value.
///
/// These are the source's faults, in the source's vocabulary.
/// [`CredentialHalt`] is what an ingest sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SecretError {
    /// The caller is not permitted to read the parameter.
    ///
    /// The loud one. P-06: this halts the pull; it never falls through to an
    /// unauthenticated request, a cached value, or a skipped vendor.
    AccessDenied,
    /// No parameter exists at that path.
    ///
    /// Distinct from [`SecretError::AccessDenied`] because the two send an
    /// operator to opposite places: denied means the role is wrong, missing
    /// means the path is.
    NotFound,
    /// The parameter exists and holds nothing.
    Empty,
    /// The source could not be reached.
    Unreachable,
}

impl fmt::Display for SecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::AccessDenied => f.write_str("access denied reading the parameter"),
            Self::NotFound => f.write_str("no parameter at that path"),
            Self::Empty => f.write_str("the parameter holds an empty value"),
            Self::Unreachable => f.write_str("the parameter store could not be reached"),
        }
    }
}

impl std::error::Error for SecretError {}

/// Why an ingest stopped.
///
/// Carries the vendor and the field **name**. It never carries the credential
/// value, and it never carries the assembled path — a path names an account,
/// and an error is the thing most likely to be logged, pasted into an issue and
/// pushed to a public repository.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CredentialHalt {
    /// The source refused, and this is where the pull stops.
    Refused {
        /// Which vendor's credential.
        vendor: Vendor,
        /// Which field.
        field: String,
        /// What the source said.
        cause: SecretError,
    },
    /// The vendor rejected the credential, and the re-read returned the same
    /// value.
    ///
    /// The token is dead and this repository is not the thing that can replace
    /// it — see this module's header, and `CLAUDE.md` §8.
    DeadToken {
        /// Which vendor's credential.
        vendor: Vendor,
        /// Which field.
        field: String,
    },
}

impl fmt::Display for CredentialHalt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused {
                vendor,
                field,
                cause,
            } => write!(f, "{} field {field}: {cause}", vendor.as_str()),
            Self::DeadToken { vendor, field } => write!(
                f,
                "{} field {field}: the vendor rejected this credential and the re-read \
                 returned the same value; it is dead and nothing here mints one",
                vendor.as_str()
            ),
        }
    }
}

impl std::error::Error for CredentialHalt {}

/// A source of credential values. One method, and it reads.
///
/// Implemented in this change by [`SsmSecretSource`] over a [`ParameterStore`],
/// and by test doubles. There is no default implementation and no blanket one:
/// a source that silently produced a value would be the fallback `CLAUDE.md` §4
/// bans, in the one place where the fallback is a credential.
pub trait SecretSource {
    /// Reads the value at `path`.
    ///
    /// # Errors
    ///
    /// [`SecretError`], in the source's own vocabulary.
    fn read(&self, path: &CredentialPath<'_>) -> Result<Secret, SecretError>;
}

/// The minimal AWS Systems Manager Parameter Store surface this crate uses.
///
/// The port an SDK client plugs into. It is deliberately *one read method*:
/// the real client offers `put_parameter`, `delete_parameter` and a dozen
/// others, and none of them can be reached through a trait that does not
/// declare them. Narrowing the surface at the port is what makes "read-only"
/// a property of the type rather than a promise about call sites.
pub trait ParameterStore {
    /// Fetches one parameter by name.
    ///
    /// `with_decryption` is passed rather than assumed because the real API
    /// takes it and defaults it to *false*, which returns the ciphertext of a
    /// `SecureString`. [`SsmSecretSource`] always passes `true`.
    ///
    /// # Errors
    ///
    /// [`SecretError`], mapped from whatever the client reported.
    fn get_parameter(&self, name: &str, with_decryption: bool) -> Result<String, SecretError>;
}

/// Reads credentials from Parameter Store through a [`ParameterStore`] client.
///
/// The whole adapter is: render the path, ask for it decrypted, wrap the answer
/// in a [`Secret`]. It exists as a named type rather than as three lines at a
/// call site so that the `with_decryption` argument has exactly one value in
/// this repository, and so that the SDK's client type never appears above this
/// line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsmSecretSource<C> {
    client: C,
    region: String,
}

impl<C> SsmSecretSource<C> {
    /// Binds a client to the region the configuration names.
    ///
    /// The region comes from [`crate::config::CredentialConfig::region`], which
    /// has already refused anything other than
    /// [`crate::config::REGION`]. It is held rather than re-derived so the
    /// value a client was built for and the value the configuration named are
    /// the same string, and [`SsmSecretSource::region`] can be asserted against
    /// the configuration in a test.
    #[must_use]
    pub fn new(client: C, region: &str) -> Self {
        Self {
            client,
            region: region.to_owned(),
        }
    }

    /// The region this source reads from.
    #[must_use]
    pub fn region(&self) -> &str {
        &self.region
    }

    /// The client, for a caller that needs to assert against it.
    ///
    /// Borrowed, never returned by value and never made mutable: this is how
    /// `pull::unit::readonly_credentials` reads the double's counters after a
    /// whole credential read, and a `&mut` here would be a door into the client
    /// that the read path does not have.
    #[must_use]
    pub const fn client(&self) -> &C {
        &self.client
    }
}

impl<C: ParameterStore> SecretSource for SsmSecretSource<C> {
    /// # Errors
    ///
    /// Whatever the client reported, or [`SecretError::Empty`] for a parameter
    /// that exists and holds nothing.
    fn read(&self, path: &CredentialPath<'_>) -> Result<Secret, SecretError> {
        // One bounded allocation, at most `config::MAX_PATH_LEN` bytes. The
        // client API takes a `&str`, and the path is assembled from borrowed
        // segments, so this is the one place the four of them become one
        // string -- and it is dropped at the end of the call.
        let name = path.to_string();
        // `true`, always. `false` returns the ciphertext of a SecureString,
        // which is a valid string that every vendor rejects as a bad
        // credential -- a failure that looks like an expired token and is not.
        //
        // There is NO check here that the value came back decrypted, and that
        // is stated rather than implied: see this module's header and
        // docs/06-limits.md section 19.
        let value = self.client.get_parameter(&name, true)?;
        Secret::new(value)
    }
}

/// Reads one vendor's credentials, and halts rather than degrading.
///
/// Thin on purpose. Everything it adds to a [`SecretSource`] is the vocabulary
/// an ingest needs — which vendor, which field — and the one rule that cannot
/// live in a source: a re-read that returns the same dead value is the end of
/// the pull.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialReader<S> {
    source: S,
}

impl<S: SecretSource> CredentialReader<S> {
    /// Wraps a source.
    #[must_use]
    pub const fn new(source: S) -> Self {
        Self { source }
    }

    /// The source, for a caller that needs to assert against it.
    #[must_use]
    pub const fn source(&self) -> &S {
        &self.source
    }

    /// Reads one credential.
    ///
    /// # Errors
    ///
    /// [`CredentialHalt::Refused`] naming the vendor, the field and the cause.
    /// There is no arm that returns a value the source did not give — P-06.
    pub fn read(
        &self,
        vendor: Vendor,
        path: &CredentialPath<'_>,
    ) -> Result<Secret, CredentialHalt> {
        self.source
            .read(path)
            .map_err(|cause| CredentialHalt::Refused {
                vendor,
                field: path.field().to_owned(),
                cause,
            })
    }

    /// Re-reads a credential the vendor has just rejected.
    ///
    /// `CLAUDE.md` §8, as a function: the stale token is re-read, and if the
    /// re-read returns the same dead value the pull halts loudly. It is never
    /// re-minted — a local mint would invalidate the token another system
    /// shares.
    ///
    /// # Errors
    ///
    /// [`CredentialHalt::DeadToken`] when the re-read returns the value that
    /// was just rejected, or [`CredentialHalt::Refused`] when the re-read
    /// itself fails.
    pub fn reread_after_rejection(
        &self,
        vendor: Vendor,
        path: &CredentialPath<'_>,
        rejected: &Secret,
    ) -> Result<Secret, CredentialHalt> {
        let fresh = self.read(vendor, path)?;
        if fresh == *rejected {
            return Err(CredentialHalt::DeadToken {
                vendor,
                field: path.field().to_owned(),
            });
        }
        Ok(fresh)
    }
}
