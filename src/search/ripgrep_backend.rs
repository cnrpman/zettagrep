use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::Deserialize;

use crate::paths;
use crate::{ZgResult, other};

use super::backend::{GrepHit, ScanBackend};

const RG_BUNDLED_RELATIVE_PATHS: &[&str] = &["rg", "../libexec/rg"];

pub struct RipgrepScanBackend;

impl ScanBackend for RipgrepScanBackend {
    fn regex_search(&self, root: &Path, pattern: &str) -> ZgResult<Vec<GrepHit>> {
        let root = paths::resolve_existing_path(root)?;
        if has_zg_component(&root) {
            return Ok(Vec::new());
        }

        let rg = resolve_rg_binary()?;
        let mut hits = run_rg(&rg, pattern, &root)?;
        hits.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.line_number.cmp(&right.line_number))
                .then_with(|| left.line.cmp(&right.line))
        });
        Ok(hits)
    }
}

fn resolve_rg_binary() -> ZgResult<OsString> {
    resolve_rg_binary_with(
        std::env::var_os("ZG_RG_BIN"),
        std::env::current_exe().ok().as_deref(),
    )
}

fn resolve_rg_binary_with(
    override_bin: Option<OsString>,
    current_exe: Option<&Path>,
) -> ZgResult<OsString> {
    if let Some(candidate) = override_bin {
        return ensure_rg_works(candidate, "ZG_RG_BIN override");
    }

    if let Some(exe) = current_exe {
        let Some(exe_dir) = exe.parent() else {
            return ensure_rg_works(OsString::from("rg"), "PATH lookup");
        };

        for relative in RG_BUNDLED_RELATIVE_PATHS {
            let candidate = exe_dir.join(relative);
            if candidate.is_file() {
                return ensure_rg_works(candidate.into_os_string(), "bundled ripgrep next to zg");
            }
        }
    }

    ensure_rg_works(OsString::from("rg"), "PATH lookup")
}

fn ensure_rg_works(candidate: OsString, source: &str) -> ZgResult<OsString> {
    let output = Command::new(&candidate).arg("--version").output();
    match output {
        Ok(output) if output.status.success() => Ok(candidate),
        Ok(output) => Err(other(format!(
            "ripgrep runtime dependency is unavailable: `{}` from {source} exited with status {status}; install ripgrep, set ZG_RG_BIN, or bundle `rg` next to `zg` / under `../libexec/rg`",
            PathBuf::from(&candidate).display(),
            status = output.status
        ))),
        Err(error) => Err(other(format!(
            "ripgrep runtime dependency is unavailable: failed to execute `{}` from {source}: {error}; install ripgrep, set ZG_RG_BIN, or bundle `rg` next to `zg` / under `../libexec/rg`",
            PathBuf::from(&candidate).display()
        ))),
    }
}

fn has_zg_component(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::Normal(name) if name == ".zg"))
}

fn run_rg(rg: &OsStr, pattern: &str, root: &Path) -> ZgResult<Vec<GrepHit>> {
    let output = Command::new(rg)
        .arg("--json")
        .arg("--line-number")
        .arg("--color")
        .arg("never")
        .arg("--glob")
        .arg("!.zg/**")
        .arg("-e")
        .arg(pattern)
        .arg("--")
        .arg(root)
        .output()?;

    let code = output.status.code();
    if !matches!(code, Some(0) | Some(1)) {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(other(format!(
            "ripgrep search failed with status {}: {}",
            output.status,
            stderr.trim()
        )));
    }

    parse_rg_json_stream(&output.stdout)
}

fn parse_rg_json_stream(stdout: &[u8]) -> ZgResult<Vec<GrepHit>> {
    let mut hits = Vec::new();
    for line in stdout.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }

        let message: RgMessage = serde_json::from_slice(line)?;
        if message.kind != "match" {
            continue;
        }

        let Some(data) = message.data else {
            continue;
        };
        let data: RgMatchData = serde_json::from_value(data)?;
        let path = decode_path(data.path)?;
        let text = decode_text(data.lines).trim_end_matches('\n').to_string();
        let line_number = data.line_number.unwrap_or(1);

        hits.push(GrepHit {
            path,
            line_number,
            line: text,
        });
    }
    Ok(hits)
}

fn decode_path(field: RgTextField) -> ZgResult<PathBuf> {
    if let Some(text) = field.text {
        return Ok(PathBuf::from(text));
    }

    let Some(bytes) = field.bytes else {
        return Err(other("ripgrep JSON event missing path"));
    };

    let raw = BASE64_STANDARD.decode(bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(PathBuf::from(std::ffi::OsString::from_vec(raw)))
    }
    #[cfg(not(unix))]
    {
        Ok(PathBuf::from(String::from_utf8_lossy(&raw).into_owned()))
    }
}

fn decode_text(field: RgTextField) -> String {
    if let Some(text) = field.text {
        return text;
    }
    if let Some(bytes) = field.bytes {
        if let Ok(raw) = BASE64_STANDARD.decode(bytes) {
            return String::from_utf8_lossy(&raw).into_owned();
        }
    }
    String::new()
}

#[derive(Debug, Deserialize)]
struct RgMessage {
    #[serde(rename = "type")]
    kind: String,
    data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct RgMatchData {
    path: RgTextField,
    lines: RgTextField,
    line_number: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct RgTextField {
    text: Option<String>,
    bytes: Option<String>,
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{RipgrepScanBackend, parse_rg_json_stream, resolve_rg_binary_with};
    use crate::search::backend::ScanBackend;

    fn temp_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("zg-scan-{name}-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rg_resolution_reports_clear_error_for_missing_binary() {
        let error = resolve_rg_binary_with(
            Some(PathBuf::from("/definitely/missing/rg").into_os_string()),
            None,
        )
        .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("ripgrep runtime dependency is unavailable"));
        assert!(message.contains("ZG_RG_BIN"));
    }

    #[test]
    fn regex_search_uses_ripgrep_style_visibility_rules() {
        let root = temp_dir("visibility");
        let child = root.join("child");
        fs::create_dir_all(&child).unwrap();
        fs::write(root.join(".ignore"), "ignored.md\n").unwrap();
        fs::write(child.join(".hidden.md"), "needle hidden").unwrap();
        fs::write(child.join("ignored.md"), "needle ignored").unwrap();
        fs::write(child.join("keep.md"), "needle visible").unwrap();

        let hits = RipgrepScanBackend.regex_search(&child, "needle").unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].path.ends_with("keep.md"));
    }

    #[test]
    fn regex_search_skips_explicit_zg_paths() {
        let root = temp_dir("zg-state");
        let hidden = root.join(".zg");
        fs::create_dir_all(&hidden).unwrap();
        let file = hidden.join("state.txt");
        fs::write(&file, "needle").unwrap();

        let hits = RipgrepScanBackend.regex_search(&file, "needle").unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn regex_search_returns_hits_sorted_by_path_and_line_number() {
        let root = temp_dir("sorted");
        fs::write(root.join("b.md"), "needle second\nneedle third").unwrap();
        fs::write(root.join("a.md"), "needle first").unwrap();

        let hits = RipgrepScanBackend.regex_search(&root, "needle").unwrap();
        let rendered = hits
            .iter()
            .map(|hit| {
                format!(
                    "{}:{}:{}",
                    hit.path.file_name().unwrap().to_string_lossy(),
                    hit.line_number,
                    hit.line
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec![
                "a.md:1:needle first",
                "b.md:1:needle second",
                "b.md:2:needle third",
            ]
        );
    }

    #[test]
    fn regex_search_on_file_scope_preserves_matching_lines() {
        let root = temp_dir("file-scope");
        let file = root.join("note.md");
        fs::write(&file, "alpha\nneedle one\nbeta\nneedle two").unwrap();

        let hits = RipgrepScanBackend.regex_search(&file, "needle").unwrap();
        let lines = hits
            .iter()
            .map(|hit| (hit.line_number, hit.line.as_str()))
            .collect::<Vec<_>>();

        assert_eq!(lines, vec![(2, "needle one"), (4, "needle two")]);
        assert!(hits.iter().all(|hit| hit.path == file));
    }

    #[test]
    fn parse_rg_json_stream_extracts_match_events() {
        let input = br#"{"type":"begin","data":{"path":{"text":"/tmp/a.txt"}}}
{"type":"match","data":{"path":{"text":"/tmp/a.txt"},"lines":{"text":"needle:x\n"},"line_number":2,"absolute_offset":4,"submatches":[]}}
{"type":"summary","data":{"stats":{}}}
"#;

        let hits = parse_rg_json_stream(input).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, PathBuf::from("/tmp/a.txt"));
        assert_eq!(hits[0].line_number, 2);
        assert_eq!(hits[0].line, "needle:x");
    }
}
