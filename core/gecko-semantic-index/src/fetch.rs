//! Model acquisition — the `gecko model fetch` machinery.
//!
//! This module is **compiled always** and is deliberately candle-free: it only
//! downloads bytes. It is the SINGLE implementation used by both the
//! `gecko model fetch` subcommand (explicit pre-stage) and the embedder's
//! `auto_fetch` path (first-embed download). Load-bearing rule #1 holds: nothing
//! here vendors weights into the build tree — every byte lands in the runtime
//! cache dir (resolved by the caller, always OUTSIDE the repo).
//!
//! ## `model_id = "<model>@<revision>"`
//! The id is BOTH the cache key and the compatibility check: the `@<revision>`
//! pins exactly which revision is fetched, so a silent upstream reweight can't
//! masquerade as the same model.
//!
//! ## Acquisition priority
//! 1. Present in the dest dir (all required files) → no-op cache hit.
//! 2. Otherwise download the required files (weights, tokenizer, config) from the
//!    source — HuggingFace Hub by default, or a configured mirror base URL —
//!    with a progress indicator, then (best-effort) verify checksums.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

/// The default upstream source: the HuggingFace Hub.
pub const DEFAULT_HF_BASE: &str = "https://huggingface.co";

/// The files GECKO needs to run a BGE/BERT embedder: the weights, the tokenizer,
/// and the model config. (No `pytorch_model.bin` — we require safetensors.)
pub const REQUIRED_FILES: [&str; 3] = ["config.json", "tokenizer.json", "model.safetensors"];

/// The Tier-3-validated, pinned `bge-large-en-v1.5` revision. This is the SINGLE
/// SOURCE OF TRUTH for the pinned SHA: `gecko-bin`'s config `default_model_id()`
/// re-uses this const, and it is kept in lockstep with `MODEL_REVISION` in
/// `.github/workflows/model-integration.yml` (where it was last validated) —
/// bumping one without the others is caught by the `model-bump-gate` CI job.
pub const PINNED_MODEL_REVISION: &str = "d4aa6901d3a41ba39fb536a557fa166f842b0e09";

/// Per-file SHA-256 digests for the pinned `bge-large-en-v1.5` revision, sourced
/// from HuggingFace at revision [`PINNED_MODEL_REVISION`] (`model.safetensors` via
/// the git-LFS pointer `oid`; the two small text files by hashing their pinned
/// content). Content-addressed by the revision, so these hold whether the model is
/// named with or without the `BAAI/` org prefix. They turn `gecko model fetch`
/// from best-effort-log into ENFORCED verification for the shipped default model.
const BGE_LARGE_EN_V15_CHECKSUMS: [(&str, &str); 3] = [
    (
        "config.json",
        "446712fac367857b4b1302762fe1cd7bfa8b3c4b77b4dc5d77c4025407660896",
    ),
    (
        "tokenizer.json",
        "d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66",
    ),
    (
        "model.safetensors",
        "45e1954914e29bd74080e6c1510165274ff5279421c89f76c418878732f64ae7",
    ),
];

/// Returns the pinned per-file SHA-256 digests for `model_id`, or `&[]` when the
/// revision is not one GECKO pins in-tree (custom / user-supplied models can't be
/// pinned here, so they fall back to best-effort hash logging). Matching is by the
/// model *basename* + revision, so both `bge-large-en-v1.5@<sha>` and
/// `BAAI/bge-large-en-v1.5@<sha>` resolve to the same content digests.
pub fn known_checksums(model_id: &str) -> &'static [(&'static str, &'static str)] {
    match parse_model_id(model_id) {
        Ok((model, revision))
            if revision == PINNED_MODEL_REVISION
                && model.rsplit('/').next() == Some("bge-large-en-v1.5") =>
        {
            &BGE_LARGE_EN_V15_CHECKSUMS
        }
        _ => &[],
    }
}

/// Outcome of a fetch: either the cache already had everything, or we downloaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchOutcome {
    /// All required files were already present in the dest dir (idempotent no-op).
    CacheHit,
    /// The listed files were downloaded into the dest dir.
    Downloaded { files: Vec<String> },
}

/// Splits a `"<model>@<revision>"` id into its two parts.
///
/// The revision is mandatory (content-addressing): an id without `@<revision>`,
/// an empty model, or an empty revision is rejected. The `@<revision>` placeholder
/// default (`"<revision>"`) is a real-but-unresolved value and is *syntactically*
/// valid here — resolving it to a concrete pinned hash is a config/release
/// concern, not this parser's job.
pub fn parse_model_id(model_id: &str) -> Result<(&str, &str)> {
    let (model, revision) = model_id.split_once('@').with_context(|| {
        format!("model_id '{model_id}' is not of the form '<model>@<revision>'")
    })?;
    if model.is_empty() {
        bail!("model_id '{model_id}' has an empty model name");
    }
    if revision.is_empty() {
        bail!("model_id '{model_id}' has an empty revision");
    }
    Ok((model, revision))
}

/// Builds the download URL for one file.
///
/// - No mirror (`source = None`) → the HuggingFace resolve URL:
///   `{DEFAULT_HF_BASE}/{model}/resolve/{revision}/{filename}`.
/// - A mirror base (`source = Some(base)`) → `{base}/{model_id}/{filename}`,
///   i.e. the mirror is expected to stage each model under its full
///   `<model>@<revision>` id (mirroring the on-disk cache layout), so an org can
///   `aws s3 sync` a populated cache dir straight to a bucket.
pub fn file_url(source: Option<&str>, model: &str, revision: &str, filename: &str) -> String {
    match source {
        Some(base) => {
            let base = base.trim_end_matches('/');
            format!("{base}/{model}@{revision}/{filename}")
        }
        None => format!("{DEFAULT_HF_BASE}/{model}/resolve/{revision}/{filename}"),
    }
}

/// Lowercase hex SHA-256 of a byte slice.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex(&hasher.finalize())
}

/// Streams a file through SHA-256 without loading it whole into memory.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file =
        std::fs::File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

/// Verifies `path`'s SHA-256 equals `expected` (case-insensitive hex). Errors with
/// the mismatch so a corrupt/tampered download is caught, not silently trusted.
pub fn verify_file_sha256(path: &Path, expected: &str) -> Result<()> {
    let got = sha256_file(path)?;
    if !got.eq_ignore_ascii_case(expected) {
        bail!(
            "checksum mismatch for {}: expected {expected}, got {got}",
            path.display()
        );
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Whether the dest dir already holds every required file as a non-empty regular
/// file (the cache-hit predicate; idempotency rests on this).
pub fn is_cached(dir: &Path) -> bool {
    REQUIRED_FILES.iter().all(|f| {
        std::fs::metadata(dir.join(f))
            .map(|m| m.is_file() && m.len() > 0)
            .unwrap_or(false)
    })
}

/// Fetches `model_id` into `dest_dir` (see the acquisition priority above).
///
/// 1. If every required file is already present → [`FetchOutcome::CacheHit`]
///    (unless `force`), so a second run is a no-op.
/// 2. Otherwise download each required file from `source` (mirror) or HuggingFace,
///    showing progress, verifying any known checksums, writing atomically.
///
/// `expected` maps filename → expected SHA-256 for ENFORCED verification (a
/// mismatch aborts the fetch); files absent from the map are hashed + logged
/// best-effort. Callers pass [`known_checksums`]`(model_id)` so the shipped
/// default model is verified, and custom models fall back to logging.
pub fn fetch_model(
    model_id: &str,
    dest_dir: &Path,
    source: Option<&str>,
    force: bool,
    expected: &[(&str, &str)],
) -> Result<FetchOutcome> {
    let (model, revision) = parse_model_id(model_id)?;

    if !force && is_cached(dest_dir) {
        return Ok(FetchOutcome::CacheHit);
    }

    std::fs::create_dir_all(dest_dir)
        .with_context(|| format!("cannot create model dir {}", dest_dir.display()))?;

    let mut downloaded = Vec::new();
    for filename in REQUIRED_FILES {
        let url = file_url(source, model, revision, filename);
        let dest = dest_dir.join(filename);
        println!("Fetching {filename} ← {url}");
        download_file(&url, &dest)
            .with_context(|| format!("failed to download {filename} from {url}"))?;

        if let Some((_, want)) = expected.iter().find(|(f, _)| *f == filename) {
            verify_file_sha256(&dest, want)
                .with_context(|| format!("checksum verification failed for {filename}"))?;
            println!("  verified {filename} (sha256 ok)");
        } else {
            let got = sha256_file(&dest)?;
            println!("  {filename} sha256: {got}");
        }
        downloaded.push(filename.to_string());
    }

    Ok(FetchOutcome::Downloaded { files: downloaded })
}

/// Downloads `url` to `dest` atomically (temp file + rename) with a progress bar.
fn download_file(url: &str, dest: &Path) -> Result<()> {
    use indicatif::{ProgressBar, ProgressStyle};

    let resp = ureq::get(url)
        .call()
        .with_context(|| format!("HTTP GET failed for {url}"))?;

    let total: u64 = resp
        .headers()
        .get("Content-Length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner} [{elapsed_precise}] [{bar:40}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})",
        )
        .unwrap_or_else(|_| ProgressStyle::default_bar())
        .progress_chars("=>-"),
    );

    // Unique per invocation (process id + a monotonic counter), not just `dest`'s
    // basename: two concurrent fetches into the same dest dir (e.g. two processes,
    // or two threads within one, racing to stage the same model) must not share a
    // temp path, or one's partial bytes corrupt the other's.
    static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = dest.with_extension(format!("part.{}.{unique}", std::process::id()));
    // Download into the unique temp; on ANY failure remove it so a retry never
    // trips over (or resumes from) a truncated partial file.
    let download = (|| -> Result<()> {
        let file = std::fs::File::create(&tmp)
            .with_context(|| format!("cannot create {}", tmp.display()))?;
        let mut writer = pb.wrap_write(std::io::BufWriter::new(file));
        let mut reader = resp.into_body().into_reader();
        std::io::copy(&mut reader, &mut writer)
            .with_context(|| format!("stream copy failed for {url}"))?;
        Ok(())
    })();
    pb.finish_and_clear();

    if let Err(e) = download {
        let _ = std::fs::remove_file(&tmp); // best-effort: don't leave an orphaned .part
        return Err(e);
    }

    std::fs::rename(&tmp, dest)
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp); // rename failed → clean up the partial
        })
        .with_context(|| format!("cannot finalize {} → {}", tmp.display(), dest.display()))?;
    Ok(())
}

/// Resolves the model directory for `model_id` under a cache root, honoring an
/// explicit override. Kept here (candle-free) so the embedder's auto_fetch path
/// and the fetch subcommand agree on where bytes live without depending on
/// gecko-bin's config module.
pub fn model_dir(cache_root: &Path, model_id: &str, override_dir: Option<&str>) -> PathBuf {
    match override_dir.filter(|s| !s.is_empty()) {
        Some(explicit) => PathBuf::from(explicit),
        None => cache_root.join("models").join(model_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_model_id_into_model_and_revision() {
        let (m, r) = parse_model_id("bge-large-en-v1.5@abc123").unwrap();
        assert_eq!(m, "bge-large-en-v1.5");
        assert_eq!(r, "abc123");
        // The unresolved default placeholder is syntactically valid.
        let (m, r) = parse_model_id("bge-large-en-v1.5@<revision>").unwrap();
        assert_eq!(m, "bge-large-en-v1.5");
        assert_eq!(r, "<revision>");
    }

    #[test]
    fn rejects_malformed_model_ids() {
        assert!(parse_model_id("no-at-sign").is_err());
        assert!(parse_model_id("@rev").is_err());
        assert!(parse_model_id("model@").is_err());
    }

    #[test]
    fn hf_url_is_the_resolve_endpoint() {
        assert_eq!(
            file_url(None, "BAAI/bge-large-en-v1.5", "main", "model.safetensors"),
            "https://huggingface.co/BAAI/bge-large-en-v1.5/resolve/main/model.safetensors"
        );
    }

    #[test]
    fn mirror_url_stages_by_full_model_id() {
        assert_eq!(
            file_url(
                Some("https://mirror.example/models/"),
                "bge-large-en-v1.5",
                "abc123",
                "config.json"
            ),
            "https://mirror.example/models/bge-large-en-v1.5@abc123/config.json"
        );
    }

    #[test]
    fn model_dir_derives_and_overrides() {
        let root = Path::new("/cache/gecko");
        assert_eq!(
            model_dir(root, "bge-large-en-v1.5@abc", None),
            PathBuf::from("/cache/gecko/models/bge-large-en-v1.5@abc")
        );
        assert_eq!(
            model_dir(root, "bge-large-en-v1.5@abc", Some("/opt/staged")),
            PathBuf::from("/opt/staged")
        );
        // Empty override is treated as unset.
        assert_eq!(
            model_dir(root, "bge-large-en-v1.5@abc", Some("")),
            PathBuf::from("/cache/gecko/models/bge-large-en-v1.5@abc")
        );
    }

    #[test]
    fn checksum_verifies_against_a_small_fixture() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("fixture.bin");
        std::fs::write(&f, b"hello gecko").unwrap();
        // Known SHA-256 of the exact bytes "hello gecko".
        let expected = sha256_hex(b"hello gecko");
        verify_file_sha256(&f, &expected).unwrap();
        // A wrong digest is rejected.
        assert!(verify_file_sha256(&f, &"0".repeat(64)).is_err());
    }

    #[test]
    fn sha256_hex_matches_known_vector() {
        // SHA-256 of the empty input — a stable, well-known vector.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn is_cached_requires_all_nonempty_files() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!is_cached(tmp.path()));
        for f in REQUIRED_FILES {
            std::fs::write(tmp.path().join(f), b"x").unwrap();
        }
        assert!(is_cached(tmp.path()));
        // An empty file does not count as cached.
        std::fs::write(tmp.path().join("config.json"), b"").unwrap();
        assert!(!is_cached(tmp.path()));
    }

    #[test]
    fn known_checksums_pins_the_default_model_with_or_without_org_prefix() {
        let pinned = format!("bge-large-en-v1.5@{PINNED_MODEL_REVISION}");
        assert_eq!(known_checksums(&pinned).len(), 3);
        // The org-prefixed id resolves to the same content digests.
        let prefixed = format!("BAAI/bge-large-en-v1.5@{PINNED_MODEL_REVISION}");
        assert_eq!(known_checksums(&prefixed), known_checksums(&pinned));
        // Every required file is covered so `fetch_model` verifies (not just logs).
        for f in REQUIRED_FILES {
            assert!(
                known_checksums(&pinned).iter().any(|(name, _)| *name == f),
                "missing pinned digest for {f}"
            );
        }
        // A different revision (or an unrelated model) is not pinned → best-effort.
        assert!(known_checksums("bge-large-en-v1.5@deadbeef").is_empty());
        assert!(known_checksums("some-other-model@abc").is_empty());
        assert!(known_checksums("not-a-valid-id").is_empty());
    }

    #[test]
    fn fetch_is_a_no_op_cache_hit_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        for f in REQUIRED_FILES {
            std::fs::write(tmp.path().join(f), b"x").unwrap();
        }
        let out = fetch_model("bge-large-en-v1.5@abc", tmp.path(), None, false, &[]).unwrap();
        assert_eq!(out, FetchOutcome::CacheHit);
    }
}
