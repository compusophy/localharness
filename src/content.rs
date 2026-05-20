//! Multimodal input primitives.
//!
//! Mirrors `google.antigravity.types.{Image, Document, Audio, Video, from_file}`.
//! Validation lists track the Python `SUPPORTED_*_MIMES` frozensets — keeping
//! the supported MIME surface identical guarantees a Rust caller's payload is
//! accepted by the same harness build the Python SDK targets.

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Document,
    Audio,
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
    pub kind: MediaKind,
    pub mime_type: String,
    pub description: Option<String>,
    pub data: Bytes,
}

impl Media {
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
    Text(String),
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
    pub parts: Vec<Part>,
}

impl Content {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            parts: vec![Part::Text(text.into())],
        }
    }

    pub fn empty() -> Self {
        Self { parts: Vec::new() }
    }

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
