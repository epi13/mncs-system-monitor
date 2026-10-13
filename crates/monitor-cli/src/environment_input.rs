//! Read one Environment-authorized Forge receipt from the private provider-input bundle.
//!
//! Environment owns source admission and canonical Store reads. This module verifies the
//! bounded transport bytes and keeps their immutable source identity with the Monitor sample;
//! it never accepts a caller-selected filesystem path.

use sha2::{Digest, Sha256};
use std::error::Error;
use std::fmt::{Display, Formatter};
#[cfg(not(unix))]
use std::fs;
use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

const INPUT_DIR_ENV: &str = "MNCS_ENV_INPUT_ARTIFACT_DIR";
const INPUT_MANIFEST_ENV: &str = "MNCS_ENV_INPUT_ARTIFACTS_JSON";
const INPUT_SCHEMA: &str = "mncs.environment.provider-input-artifacts/1";
const FORGE_CAPABILITY: &str = "mncs-forge:provider-artifact-execution";
const INPUT_NAME: &str = "forge-receipt";
const MAX_MANIFEST_BYTES: usize = 32 * 1024;
const MAX_INPUT_ARTIFACTS: usize = 1;
const MAX_INPUT_ARTIFACT_BYTES: usize = 1024 * 1024;

#[derive(Debug)]
pub struct ProviderInputError(String);

impl Display for ProviderInputError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for ProviderInputError {}

#[derive(Debug)]
pub struct VerifiedForgeReceipt {
    pub content: Vec<u8>,
    pub provenance: serde_json::Value,
}

fn invalid(message: impl Into<String>) -> ProviderInputError {
    ProviderInputError(message.into())
}

fn valid_token(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.is_ascii()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
}

fn safe_relative(value: &str, field: &str) -> Result<PathBuf, ProviderInputError> {
    if value.is_empty() || value.len() > 1024 || value.contains('\\') || value.contains('\0') {
        return Err(invalid(format!("provider input {field} is invalid")));
    }
    let parts: Vec<&str> = value.split('/').collect();
    if parts
        .iter()
        .any(|part| part.is_empty() || *part == "." || *part == "..")
    {
        return Err(invalid(format!(
            "provider input {field} escapes its bundle"
        )));
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(invalid(format!(
            "provider input {field} escapes its bundle"
        )));
    }
    Ok(path.to_path_buf())
}

fn expected_field<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    name: &str,
) -> Result<&'a serde_json::Value, ProviderInputError> {
    object
        .get(name)
        .ok_or_else(|| invalid(format!("provider input field {name} is missing")))
}

fn expected_string<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    name: &str,
) -> Result<&'a str, ProviderInputError> {
    expected_field(object, name)?
        .as_str()
        .ok_or_else(|| invalid(format!("provider input field {name} is invalid")))
}

#[cfg(unix)]
fn read_only_regular_file(
    root: &Path,
    relative: &Path,
    expected_size: usize,
) -> Result<Vec<u8>, ProviderInputError> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    if !root.is_absolute() {
        return Err(invalid("provider input root is not absolute"));
    }
    let mut directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open("/")
        .map_err(|error| invalid(format!("provider input root is unavailable: {error}")))?;

    for component in root.components() {
        let Component::Normal(part) = component else {
            if matches!(component, Component::RootDir) {
                continue;
            }
            return Err(invalid("provider input root contains an unsafe component"));
        };
        let name = CString::new(part.as_bytes())
            .map_err(|_| invalid("provider input root contains an invalid component"))?;
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(invalid(
                "provider input root is unavailable or contains a symlink",
            ));
        }
        directory = unsafe { std::fs::File::from_raw_fd(fd) };
    }

    let components: Vec<_> = relative.components().collect();
    let (filename, parents) = components
        .split_last()
        .ok_or_else(|| invalid("provider input local path is invalid"))?;
    for component in parents {
        let Component::Normal(part) = component else {
            return Err(invalid("provider input local path is invalid"));
        };
        let name = CString::new(part.as_bytes())
            .map_err(|_| invalid("provider input local path is invalid"))?;
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(invalid(
                "provider input path is unavailable or contains a symlink",
            ));
        }
        directory = unsafe { std::fs::File::from_raw_fd(fd) };
    }
    let Component::Normal(filename) = filename else {
        return Err(invalid("provider input local path is invalid"));
    };
    let name = CString::new(filename.as_bytes())
        .map_err(|_| invalid("provider input local path is invalid"))?;
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        return Err(invalid(
            "provider input file is unavailable or is a symlink",
        ));
    }
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    let before = file
        .metadata()
        .map_err(|error| invalid(format!("provider input file cannot be inspected: {error}")))?;
    if !before.is_file() || before.len() as usize != expected_size {
        return Err(invalid("provider input file is not a bounded regular file"));
    }
    if before.permissions().mode() & 0o222 != 0 {
        return Err(invalid("provider input file is writable"));
    }
    let mut content = Vec::with_capacity(expected_size.saturating_add(1));
    file.by_ref()
        .take((MAX_INPUT_ARTIFACT_BYTES + 1) as u64)
        .read_to_end(&mut content)
        .map_err(|error| invalid(format!("provider input file cannot be read: {error}")))?;
    let after = file
        .metadata()
        .map_err(|error| invalid(format!("provider input file cannot be inspected: {error}")))?;
    if content.len() != expected_size || file_identity_changed(&before, &after) {
        return Err(invalid("provider input file changed while it was read"));
    }
    Ok(content)
}

#[cfg(not(unix))]
fn read_only_regular_file(
    root: &Path,
    relative: &Path,
    expected_size: usize,
) -> Result<Vec<u8>, ProviderInputError> {
    let raw_root_metadata = fs::symlink_metadata(root)
        .map_err(|error| invalid(format!("provider input directory is unavailable: {error}")))?;
    if raw_root_metadata.file_type().is_symlink() || !raw_root_metadata.is_dir() {
        return Err(invalid("provider input root is not a directory"));
    }
    let mut path = root.to_path_buf();
    for (index, component) in relative.components().enumerate() {
        let Component::Normal(part) = component else {
            return Err(invalid("provider input local path is invalid"));
        };
        path.push(part);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| invalid(format!("provider input path is unavailable: {error}")))?;
        if metadata.file_type().is_symlink()
            || (index + 1 < relative.components().count() && !metadata.is_dir())
        {
            return Err(invalid("provider input path contains an unsafe component"));
        }
        if index + 1 == relative.components().count()
            && (!metadata.is_file() || metadata.len() as usize != expected_size)
        {
            return Err(invalid("provider input file is not a bounded regular file"));
        }
    }
    let mut file = OpenOptions::new()
        .read(true)
        .open(&path)
        .map_err(|error| invalid(format!("provider input file is unavailable: {error}")))?;
    let before = file
        .metadata()
        .map_err(|error| invalid(format!("provider input file cannot be inspected: {error}")))?;
    if !before.is_file() || before.len() as usize != expected_size {
        return Err(invalid("provider input file changed before read"));
    }
    if !before.permissions().readonly() {
        return Err(invalid("provider input file is writable"));
    }
    let mut content = Vec::with_capacity(expected_size.saturating_add(1));
    file.by_ref()
        .take((MAX_INPUT_ARTIFACT_BYTES + 1) as u64)
        .read_to_end(&mut content)
        .map_err(|error| invalid(format!("provider input file cannot be read: {error}")))?;
    let after = file
        .metadata()
        .map_err(|error| invalid(format!("provider input file cannot be inspected: {error}")))?;
    if content.len() != expected_size || file_identity_changed(&before, &after) {
        return Err(invalid("provider input file changed while it was read"));
    }
    Ok(content)
}

#[cfg(unix)]
fn file_identity_changed(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    (
        before.dev(),
        before.ino(),
        before.len(),
        before.mtime(),
        before.mtime_nsec(),
        before.ctime(),
        before.ctime_nsec(),
    ) != (
        after.dev(),
        after.ino(),
        after.len(),
        after.mtime(),
        after.mtime_nsec(),
        after.ctime(),
        after.ctime_nsec(),
    )
}

#[cfg(not(unix))]
fn file_identity_changed(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    before.len() != after.len() || before.modified().ok() != after.modified().ok()
}

fn sha256_hex(content: &[u8]) -> String {
    let digest = Sha256::digest(content);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Read and verify the one optional Forge receipt selected by Environment.
/// An absent/empty Environment input bundle keeps host-only sampling available.
pub fn read_forge_receipt() -> Result<Option<VerifiedForgeReceipt>, ProviderInputError> {
    let (raw_root, raw_manifest) = match (
        std::env::var_os(INPUT_DIR_ENV),
        std::env::var(INPUT_MANIFEST_ENV).ok(),
    ) {
        (None, None) => return Ok(None),
        (Some(root), Some(manifest)) => (root, manifest),
        _ => return Err(invalid("Environment provider input bundle is incomplete")),
    };
    if raw_manifest.is_empty() && raw_root.is_empty() {
        return Ok(None);
    }
    if raw_manifest.len() > MAX_MANIFEST_BYTES {
        return Err(invalid(
            "Environment provider input manifest exceeds its size bound",
        ));
    }
    let manifest: serde_json::Value = serde_json::from_str(&raw_manifest).map_err(|error| {
        invalid(format!(
            "Environment provider input manifest is malformed: {error}"
        ))
    })?;
    let manifest_object = manifest
        .as_object()
        .ok_or_else(|| invalid("Environment provider input manifest is invalid"))?;
    if manifest_object.len() != 3
        || expected_string(manifest_object, "schema_version")? != INPUT_SCHEMA
    {
        return Err(invalid(
            "Environment provider input manifest schema is unsupported",
        ));
    }
    let session_id = expected_string(manifest_object, "session_id")?;
    if !session_id.starts_with("ses_")
        || session_id.len() > 160
        || !session_id[4..]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(invalid(
            "Environment provider input session identity is invalid",
        ));
    }
    let rows = expected_field(manifest_object, "artifacts")?
        .as_array()
        .ok_or_else(|| invalid("Environment provider input artifact list is invalid"))?;
    if rows.len() > MAX_INPUT_ARTIFACTS {
        return Err(invalid(
            "Environment provider input count exceeds its bound",
        ));
    }
    if rows.is_empty() {
        return Ok(None);
    }
    if rows.len() != 1 {
        return Err(invalid(
            "resource samples accept at most one Environment receipt input",
        ));
    }

    let artifact = rows[0]
        .as_object()
        .ok_or_else(|| invalid("Environment provider input entry is malformed"))?;
    const FIELDS: [&str; 8] = [
        "name",
        "capability",
        "binding_id",
        "store_reference",
        "path",
        "byte_count",
        "sha256",
        "local_path",
    ];
    if artifact.len() != FIELDS.len() || FIELDS.iter().any(|field| !artifact.contains_key(*field)) {
        return Err(invalid("Environment provider input entry is malformed"));
    }
    let name = expected_string(artifact, "name")?;
    if name != INPUT_NAME {
        return Err(invalid("Environment input is not named forge-receipt"));
    }
    let capability = expected_string(artifact, "capability")?;
    if capability != FORGE_CAPABILITY {
        return Err(invalid(
            "Environment receipt input is not bound to the Forge artifact capability",
        ));
    }
    let binding_id = expected_string(artifact, "binding_id")?;
    if !binding_id.starts_with("cap_")
        || binding_id.len() > 128
        || !binding_id[4..]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(invalid(
            "Environment provider input binding identity is invalid",
        ));
    }
    let store_reference = expected_string(artifact, "store_reference")?;
    if !store_reference.starts_with("mncs-store:") || !valid_token(store_reference, 512) {
        return Err(invalid(
            "Environment provider input Store reference is invalid",
        ));
    }
    let source_path = expected_string(artifact, "path")?;
    safe_relative(source_path, "source path")?;
    let local_path = safe_relative(expected_string(artifact, "local_path")?, "local path")?;
    let byte_count = expected_field(artifact, "byte_count")?
        .as_u64()
        .and_then(|count| usize::try_from(count).ok())
        .filter(|count| *count <= MAX_INPUT_ARTIFACT_BYTES)
        .ok_or_else(|| invalid("Environment provider input size exceeds its bound"))?;
    let expected_digest = expected_string(artifact, "sha256")?;
    if expected_digest.len() != 64
        || !expected_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(invalid("Environment provider input digest is invalid"));
    }
    let root = PathBuf::from(raw_root);
    if !root.is_absolute() {
        return Err(invalid(
            "Environment provider input directory must be absolute",
        ));
    }
    let content = read_only_regular_file(&root, &local_path, byte_count)?;
    let digest = sha256_hex(&content);
    if digest != expected_digest {
        return Err(invalid(
            "Environment provider input failed SHA-256 verification",
        ));
    }
    let provenance = serde_json::json!({
        "environment_session_id": session_id,
        "name": name,
        "capability": capability,
        "binding_id": binding_id,
        "store_reference": store_reference,
        "path": source_path,
        "byte_count": byte_count,
        "sha256": digest,
    });
    Ok(Some(VerifiedForgeReceipt {
        content,
        provenance,
    }))
}
