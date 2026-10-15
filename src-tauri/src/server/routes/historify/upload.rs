//! The file upload route (web `historify.py` `/upload`): a CSV or Parquet
//! file of candles for one symbol, streamed to a temporary file under the
//! size limit, then imported.

use super::{Ctx, MAX_UPLOAD_BYTES};
use crate::historify::import::FileKind;
use crate::server::api_v1::send;
use crate::server::envelope::error;
use axum::{
    extract::{Multipart, State},
    http::StatusCode,
    response::Response,
};
use std::collections::HashMap;
use std::path::PathBuf;

/// Removes the saved upload on every path.
struct UploadFile(PathBuf);

impl Drop for UploadFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn unreadable_upload() -> Response {
    error(
        StatusCode::BAD_REQUEST,
        "The upload could not be read. Try again.",
    )
}

/// POST /historify/api/upload (multipart: file, symbol, exchange, interval)
pub async fn upload(State(ctx): Ctx, mut mp: Multipart) -> Response {
    use tokio::io::AsyncWriteExt;
    let mut saved: Option<(UploadFile, FileKind)> = None;
    let mut fields: HashMap<String, String> = HashMap::new();
    let mut file_seen = false;
    let mut bad_name: Option<Response> = None;
    loop {
        let mut field = match mp.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
                    return too_large();
                }
                return unreadable_upload();
            }
        };
        let name = field.name().unwrap_or_default().to_string();
        if name == "file" && !file_seen {
            file_seen = true;
            let filename = field.file_name().unwrap_or_default().to_lowercase();
            let kind = if filename.is_empty() {
                bad_name = Some(error(StatusCode::BAD_REQUEST, "No file selected"));
                None
            } else if filename.ends_with(".csv") {
                Some(FileKind::Csv)
            } else if filename.ends_with(".parquet") {
                Some(FileKind::Parquet)
            } else {
                bad_name = Some(error(
                    StatusCode::BAD_REQUEST,
                    "File must be CSV or Parquet",
                ));
                None
            };
            let Some(kind) = kind else { continue };
            let dir = match ctx.historify.work_dir() {
                Ok(d) => d.to_path_buf(),
                Err(e) => {
                    tracing::error!("Could not create the Historify work folder: {}", e);
                    return error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Could not save the upload. Check free disk space.",
                    );
                }
            };
            let ext = if kind == FileKind::Csv {
                "csv"
            } else {
                "parquet"
            };
            let path = dir.join(format!(
                "historify_upload_{}.{}",
                uuid::Uuid::new_v4().simple(),
                ext
            ));
            let guard = UploadFile(path.clone());
            let mut out = match tokio::fs::File::create(&path).await {
                Ok(f) => f,
                Err(e) => {
                    tracing::error!("Could not save an upload: {}", e);
                    return error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Could not save the upload. Check free disk space.",
                    );
                }
            };
            let mut size = 0usize;
            loop {
                match field.chunk().await {
                    Ok(Some(chunk)) => {
                        size += chunk.len();
                        if size > MAX_UPLOAD_BYTES {
                            return too_large();
                        }
                        if out.write_all(&chunk).await.is_err() {
                            return error(
                                StatusCode::INTERNAL_SERVER_ERROR,
                                "Could not save the upload. Check free disk space.",
                            );
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
                            return too_large();
                        }
                        return unreadable_upload();
                    }
                }
            }
            if out.flush().await.is_err() {
                return unreadable_upload();
            }
            drop(out);
            saved = Some((guard, kind));
        } else if !name.is_empty() {
            match field.text().await {
                Ok(t) => {
                    fields.insert(name, t);
                }
                Err(_) => return unreadable_upload(),
            }
        }
    }
    if !file_seen {
        return error(StatusCode::BAD_REQUEST, "No file provided");
    }
    if let Some(r) = bad_name {
        return r;
    }
    let Some((file, kind)) = saved else {
        return error(StatusCode::BAD_REQUEST, "No file provided");
    };
    let f = |k: &str| {
        fields
            .get(k)
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };
    let (symbol, exchange, interval) = (
        f("symbol").to_uppercase(),
        f("exchange").to_uppercase(),
        f("interval"),
    );
    if symbol.is_empty() || exchange.is_empty() || interval.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "Symbol, exchange, and interval are required",
        );
    }
    let r = ctx
        .historify
        .upload(&file.0, kind, &symbol, &exchange, &interval)
        .await;
    drop(file);
    send(r)
}

fn too_large() -> Response {
    error(
        StatusCode::BAD_REQUEST,
        format!(
            "File too large. Maximum size is {} MB",
            MAX_UPLOAD_BYTES / (1024 * 1024)
        ),
    )
}
