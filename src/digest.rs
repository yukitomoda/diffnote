//! Content digests (`sha256:<hex>`), the one form used everywhere a digest
//! is stored or compared.

use sha2::{Digest, Sha256};

pub fn digest(bytes: impl AsRef<[u8]>) -> String {
    let hash = Sha256::digest(bytes.as_ref());
    format!("sha256:{hash:x}")
}
