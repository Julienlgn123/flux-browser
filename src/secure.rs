//! Chiffrement des données locales avec DPAPI : la clé est liée à la session
//! Windows de l'utilisateur, un autre compte ou une autre machine ne peut pas
//! relire les fichiers. Même mécanisme que Chrome/Edge pour leurs secrets.

use windows::Win32::{
    Foundation::{LocalFree, HLOCAL},
    Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB},
};

const MAGIC: &[u8] = b"FLX1";
/// Entropie propre à Flux : un autre logiciel de la même session ne peut pas
/// déchiffrer ces fichiers par simple appel DPAPI.
const ENTROPY: &[u8] = b"Flux/2026/local-data";

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
}

unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
    let _ = LocalFree(Some(HLOCAL(out.pbData as _)));
    v
}

pub fn encrypt(plain: &[u8]) -> Option<Vec<u8>> {
    unsafe {
        let mut out = CRYPT_INTEGER_BLOB::default();
        let entropy = blob(ENTROPY);
        CryptProtectData(&blob(plain), None, Some(&entropy), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out).ok()?;
        let mut v = MAGIC.to_vec();
        v.extend(take(out));
        Some(v)
    }
}

/// Déchiffre un fichier Flux. Les anciens fichiers en clair sont acceptés tels quels
/// (ils seront réécrits chiffrés à la prochaine sauvegarde).
pub fn decrypt(data: &[u8]) -> Option<Vec<u8>> {
    let Some(cipher) = data.strip_prefix(MAGIC) else {
        return Some(data.to_vec());
    };
    unsafe {
        let mut out = CRYPT_INTEGER_BLOB::default();
        let entropy = blob(ENTROPY);
        CryptUnprotectData(&blob(cipher), None, Some(&entropy), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out).ok()?;
        Some(take(out))
    }
}
