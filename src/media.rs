//! Fond personnalisé de la page d'accueil (image, GIF ou vidéo).
//!
//! Le fichier choisi est copié dans %LOCALAPPDATA%\Flux puis servi à l'interface par le
//! protocole `flux://`. Les vidéos sont lues par morceaux (requêtes `Range`) : seule la
//! portion en cours de lecture est en mémoire, quelle que soit la taille du fichier.

use std::{
    borrow::Cow,
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use wry::http::{header, Request, Response, StatusCode};

const PREFIX: &str = "background.";
/// Taille maximale d'une réponse partielle.
const CHUNK: u64 = 2 << 20;

pub fn mime(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        _ => "application/octet-stream",
    }
}

/// Copie le fichier choisi comme nouveau fond et renvoie son nom (ex. `background.mp4`).
pub fn install(dir: &Path, source: &Path) -> Option<String> {
    let ext = source.extension()?.to_str()?.to_ascii_lowercase();
    if mime(&ext) == "application/octet-stream" {
        return None;
    }
    clear(dir);
    let name = format!("{PREFIX}{ext}");
    fs::copy(source, dir.join(&name)).ok()?;
    Some(name)
}

/// Supprime l'ancien fond.
pub fn clear(dir: &Path) {
    if let Ok(entries) = fs::read_dir(dir) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().starts_with(PREFIX) {
                let _ = fs::remove_file(e.path());
            }
        }
    }
}

fn status(code: StatusCode) -> Response<Cow<'static, [u8]>> {
    Response::builder().status(code).body(Cow::Borrowed(&[][..])).unwrap()
}

pub fn serve(dir: &PathBuf, req: Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let name = req.uri().path().trim_start_matches('/');
    // Seuls les fichiers de fond sont accessibles, jamais le reste du dossier Flux.
    if !name.starts_with(PREFIX) || name.contains(['/', '\\']) || name.contains("..") {
        return status(StatusCode::NOT_FOUND);
    }
    let path = dir.join(name);
    let Ok(mut file) = File::open(&path) else { return status(StatusCode::NOT_FOUND) };
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let ext = name.rsplit('.').next().unwrap_or("");
    let range = req
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes="))
        .and_then(|v| v.split_once('-'))
        .map(|(a, b)| (a.parse::<u64>().unwrap_or(0), b.parse::<u64>().ok()));

    let (start, end, partial) = match range {
        Some((a, b)) if a < size => (a, b.unwrap_or(size - 1).min(size - 1).min(a + CHUNK - 1), true),
        Some(_) => return status(StatusCode::RANGE_NOT_SATISFIABLE),
        None if mime(ext).starts_with("video/") && size > CHUNK => (0, CHUNK - 1, true),
        None => (0, size.saturating_sub(1), false),
    };
    let mut body = vec![0u8; (end + 1 - start) as usize];
    if file.seek(SeekFrom::Start(start)).is_err() || file.read_exact(&mut body).is_err() {
        return status(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let mut res = Response::builder()
        .header(header::CONTENT_TYPE, mime(ext))
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*");
    if partial {
        res = res
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"));
    }
    res.body(Cow::Owned(body)).unwrap()
}
