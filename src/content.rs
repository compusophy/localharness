//! Multimodal input primitives (text, images, documents, audio, video).
//!
//! These are the harness's own provider-neutral input types — the data model
//! every LLM backend converts FROM. The validation lists below enumerate the
//! MIME types accepted by the supported backends.

use std::path::{Path, PathBuf};

use bytes::Bytes;

use crate::error::{Error, Result};

const SUPPORTED_IMAGE: &[&str] = &["image/bmp", "image/jpeg", "image/png", "image/webp"];

const SUPPORTED_DOCUMENT: &[&str] = &[
    "application/pdf",
    "application/json",
    "text/css",
    "text/csv",
    "text/html",
    "text/javascript",
    "text/plain",
    "text/rtf",
    "text/xml",
];

const SUPPORTED_AUDIO: &[&str] = &[
    "audio/wav",
    "audio/mp3",
    "audio/aac",
    "audio/ogg",
    "audio/flac",
    "audio/opus",
    "audio/mpeg",
    "audio/m4a",
    "audio/l16",
];

const SUPPORTED_VIDEO: &[&str] = &[
    "video/3gpp",
    "video/avi",
    "video/mp4",
    "video/mpeg",
    "video/mpg",
    "video/quicktime",
    "video/webm",
    "video/wmv",
    "video/x-flv",
];

/// The category of a binary media attachment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    /// Raster image (BMP, JPEG, PNG, WebP).
    Image,
    /// Text document (PDF, JSON, HTML, CSV, etc.).
    Document,
    /// Audio clip (WAV, MP3, AAC, OGG, etc.).
    Audio,
    /// Video file (MP4, WebM, AVI, etc.).
    Video,
}

impl MediaKind {
    fn supported_mimes(self) -> &'static [&'static str] {
        match self {
            Self::Image => SUPPORTED_IMAGE,
            Self::Document => SUPPORTED_DOCUMENT,
            Self::Audio => SUPPORTED_AUDIO,
            Self::Video => SUPPORTED_VIDEO,
        }
    }

    fn from_mime(mime: &str) -> Option<Self> {
        if SUPPORTED_IMAGE.contains(&mime) {
            Some(Self::Image)
        } else if SUPPORTED_DOCUMENT.contains(&mime) {
            Some(Self::Document)
        } else if SUPPORTED_AUDIO.contains(&mime) {
            Some(Self::Audio)
        } else if SUPPORTED_VIDEO.contains(&mime) {
            Some(Self::Video)
        } else {
            None
        }
    }
}

/// A binary attachment with a declared MIME type.
///
/// Internally stored as `Bytes` so cloning the part into multiple stream
/// frames is reference-counted — a multi-megabyte PDF is never copied.
#[derive(Debug, Clone)]
pub struct Media {
    /// Category (image, document, audio, video).
    pub kind: MediaKind,
    /// MIME type string (e.g. `"image/png"`).
    pub mime_type: String,
    /// Optional human-readable description for the model.
    pub description: Option<String>,
    /// Raw binary content (reference-counted; cloning is cheap).
    pub data: Bytes,
}

impl Media {
    /// Create a media attachment, validating the MIME type against the kind.
    pub fn new(
        kind: MediaKind,
        mime_type: impl Into<String>,
        data: impl Into<Bytes>,
    ) -> Result<Self> {
        let mime_type = mime_type.into();
        if !kind.supported_mimes().contains(&mime_type.as_str()) {
            return Err(Error::config(format!(
                "unsupported {:?} MIME type: '{mime_type}'",
                kind
            )));
        }
        Ok(Self {
            kind,
            mime_type,
            description: None,
            data: data.into(),
        })
    }

    /// Attach a human-readable description for the model.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Load a file from disk, infer its MIME type from the extension, and
    /// build the matching `Media` variant. Mirrors Python `from_file()`.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path: PathBuf = path.as_ref().to_path_buf();
        let bytes = std::fs::read(&path)?;
        let mime = guess_mime_from_extension(&path).ok_or_else(|| {
            Error::config(format!(
                "could not infer MIME for path: '{}'",
                path.display()
            ))
        })?;
        let kind = MediaKind::from_mime(&mime)
            .ok_or_else(|| Error::config(format!("unsupported MIME type: '{mime}'")))?;
        Self::new(kind, mime, bytes)
    }
}

fn guess_mime_from_extension(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let mime = match ext.as_str() {
        // images
        "bmp" => "image/bmp",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        // documents
        "pdf" => "application/pdf",
        "json" => "application/json",
        "css" => "text/css",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "js" | "mjs" => "text/javascript",
        "txt" | "md" | "log" => "text/plain",
        "rtf" => "text/rtf",
        "xml" => "text/xml",
        // audio
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        "aac" => "audio/aac",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "opus" => "audio/opus",
        "m4a" => "audio/m4a",
        // video
        "3gp" => "video/3gpp",
        "avi" => "video/avi",
        "mp4" => "video/mp4",
        "mpeg" | "mpg" => "video/mpeg",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "wmv" => "video/wmv",
        "flv" => "video/x-flv",
        _ => return None,
    };
    Some(mime.to_string())
}

/// A single textual or media part of a prompt.
#[derive(Debug, Clone)]
pub enum Part {
    /// Plain text content.
    Text(String),
    /// Binary media attachment.
    Media(Media),
}

impl From<String> for Part {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<&str> for Part {
    fn from(text: &str) -> Self {
        Self::Text(text.to_string())
    }
}

impl From<Media> for Part {
    fn from(m: Media) -> Self {
        Self::Media(m)
    }
}

/// A complete prompt payload — one or more `Part`s.
///
/// The `From` impls let callers write `agent.chat("hello")`,
/// `agent.chat(vec!["look at:".into(), image.into()])`, or build a
/// `Content` explicitly.
#[derive(Debug, Clone, Default)]
pub struct Content {
    /// The ordered parts composing this prompt.
    pub parts: Vec<Part>,
}

impl Content {
    /// Build a text-only prompt from a single string.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            parts: vec![Part::Text(text.into())],
        }
    }

    /// Build an empty prompt with no parts.
    pub fn empty() -> Self {
        Self { parts: Vec::new() }
    }

    /// Append a part (text or media) to this prompt.
    pub fn push(&mut self, part: impl Into<Part>) {
        self.parts.push(part.into());
    }

    /// Returns `true` iff every part is text.
    pub fn is_text_only(&self) -> bool {
        self.parts.iter().all(|p| matches!(p, Part::Text(_)))
    }

    /// Concatenate every text part with no separator. Returns `None` if any
    /// part is non-textual.
    pub fn as_text(&self) -> Option<String> {
        if !self.is_text_only() {
            return None;
        }
        let mut out = String::new();
        for p in &self.parts {
            if let Part::Text(t) = p {
                out.push_str(t);
            }
        }
        Some(out)
    }
}

impl From<&str> for Content {
    fn from(s: &str) -> Self {
        Self::text(s)
    }
}

impl From<String> for Content {
    fn from(s: String) -> Self {
        Self::text(s)
    }
}

impl From<Media> for Content {
    fn from(m: Media) -> Self {
        Self {
            parts: vec![Part::Media(m)],
        }
    }
}

impl<T> From<Vec<T>> for Content
where
    T: Into<Part>,
{
    fn from(parts: Vec<T>) -> Self {
        Self {
            parts: parts.into_iter().map(Into::into).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Media::new` validates the MIME against the declared kind: a matching MIME
    /// is accepted, a mismatched one (audio MIME under `Image`) is rejected with a
    /// message naming the offending type.
    #[test]
    fn media_new_validates_mime_against_kind() {
        assert!(Media::new(MediaKind::Image, "image/png", Vec::<u8>::new()).is_ok());
        let err = Media::new(MediaKind::Image, "audio/wav", Vec::<u8>::new()).unwrap_err();
        assert!(err.to_string().contains("audio/wav"), "{err}");
    }

    /// `from_mime` maps a representative MIME of each category to its kind, and an
    /// unknown MIME to `None`.
    #[test]
    fn from_mime_classifies_each_category() {
        assert_eq!(MediaKind::from_mime("image/png"), Some(MediaKind::Image));
        assert_eq!(MediaKind::from_mime("application/pdf"), Some(MediaKind::Document));
        assert_eq!(MediaKind::from_mime("audio/opus"), Some(MediaKind::Audio));
        assert_eq!(MediaKind::from_mime("video/mp4"), Some(MediaKind::Video));
        assert_eq!(MediaKind::from_mime("application/zip"), None);
    }

    /// THE `from_path` consistency invariant: every extension the guesser knows
    /// must resolve to a MIME that BOTH `from_mime` classifies AND `Media::new`
    /// accepts. Otherwise `Media::from_path` would guess a MIME it then rejects —
    /// a file it claims to support would fail to load. Guards the two tables
    /// (extension→MIME and the SUPPORTED_* lists) against drifting apart.
    #[test]
    fn every_guessable_extension_maps_to_an_accepted_mime() {
        const EXTS: &[&str] = &[
            "bmp", "jpg", "jpeg", "png", "webp", // images
            "pdf", "json", "css", "csv", "html", "htm", "js", "mjs", "txt", "md", "log",
            "rtf", "xml", // documents
            "wav", "mp3", "aac", "ogg", "flac", "opus", "m4a", // audio
            "3gp", "avi", "mp4", "mpeg", "mpg", "mov", "webm", "wmv", "flv", // video
        ];
        for ext in EXTS {
            let path = PathBuf::from(format!("file.{ext}"));
            let mime = guess_mime_from_extension(&path)
                .unwrap_or_else(|| panic!("no MIME guessed for .{ext}"));
            let kind = MediaKind::from_mime(&mime)
                .unwrap_or_else(|| panic!(".{ext} guessed unclassified MIME {mime}"));
            assert!(
                Media::new(kind, mime.clone(), Vec::<u8>::new()).is_ok(),
                ".{ext} guessed MIME {mime} that Media::new rejects",
            );
        }
        // An unknown extension yields no guess (not a wrong one).
        assert_eq!(guess_mime_from_extension(&PathBuf::from("file.xyz")), None);
        assert_eq!(guess_mime_from_extension(&PathBuf::from("noext")), None);
    }

    /// Case-insensitive extension matching (a `.PNG` upload is still an image).
    #[test]
    fn extension_guess_is_case_insensitive() {
        assert_eq!(
            guess_mime_from_extension(&PathBuf::from("PHOTO.PNG")).as_deref(),
            Some("image/png")
        );
    }

    /// `Content` text helpers: a text-only payload concatenates via `as_text`; a
    /// payload carrying media is not text-only and `as_text` returns `None`.
    #[test]
    fn content_text_helpers() {
        let mut c = Content::text("hello ");
        c.push("world");
        assert!(c.is_text_only());
        assert_eq!(c.as_text().as_deref(), Some("hello world"));

        let media = Media::new(MediaKind::Image, "image/png", Vec::<u8>::new()).unwrap();
        c.push(media);
        assert!(!c.is_text_only());
        assert_eq!(c.as_text(), None);

        assert!(Content::empty().is_text_only()); // vacuously text-only
    }

    /// The ergonomic `From` impls callers rely on (`agent.chat("hi")`,
    /// `agent.chat(vec![...])`).
    #[test]
    fn content_from_impls_build_expected_parts() {
        assert_eq!(Content::from("hi").parts.len(), 1);
        let mixed: Content = vec![Part::from("a"), Part::from("b")].into();
        assert_eq!(mixed.parts.len(), 2);
        assert!(mixed.is_text_only());
    }
}
