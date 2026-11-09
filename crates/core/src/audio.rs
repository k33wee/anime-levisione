//! Read audio-language metadata without opening a player. HLS masters need only
//! one HTTP request; other media use ffprobe when it is installed.
use crate::{
    diagnostics::{DiagnosticFailure, DiagnosticOperation},
    media_error::{self, MediaError},
    model::clean,
    provider::{Media, http::Http},
};
use anyhow::{Context, Result, bail};
use regex::Regex;
use serde_json::Value;
use std::{collections::HashMap, process::Stdio, sync::LazyLock, time::Duration};
use tokio::{io::AsyncReadExt, process::Command};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioTrack {
    /// Normalized language code accepted by VLC and mpv, when supplied by the source.
    pub language: Option<String>,
    pub label: String,
    /// Zero-based audio ordinal (not a container's global stream index).
    pub index: usize,
    pub default: bool,
}
impl AudioTrack {
    fn new(language: &str, name: &str, index: usize, default: bool) -> Self {
        let language = language_code(language);
        let name: String = clean(name).chars().take(120).collect();
        let label = if !name.is_empty() {
            name
        } else {
            match language.as_deref() {
                Some("ita") => "Italian".into(),
                Some("eng") => "English".into(),
                Some("jpn") => "Japanese".into(),
                Some("fra") => "French".into(),
                Some("deu") => "German".into(),
                Some("spa") => "Spanish".into(),
                Some(code) => code.to_uppercase(),
                None => format!("Audio track {}", index + 1),
            }
        };
        Self {
            language,
            label,
            index,
            default,
        }
    }
}

fn language_code(value: &str) -> Option<String> {
    let value = value.trim().to_ascii_lowercase();
    let primary = value.split('-').next().unwrap_or_default();
    if !matches!(primary.len(), 2 | 3) || !primary.bytes().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    Some(
        match primary {
            "it" => "ita",
            "en" => "eng",
            "ja" => "jpn",
            "fr" | "fre" => "fra",
            "de" | "ger" => "deu",
            "es" => "spa",
            "pt" => "por",
            "zh" | "chi" => "zho",
            "ko" => "kor",
            "ru" => "rus",
            "uk" => "ukr",
            "und" => return None,
            other => other,
        }
        .into(),
    )
}

/// A language may be repeated across quality/audio groups. The picker selects
/// languages, not HLS encodings of the same language. Keep unlabelled tracks.
fn languages(tracks: impl IntoIterator<Item = AudioTrack>) -> Vec<AudioTrack> {
    let mut out: Vec<AudioTrack> = Vec::new();
    for track in tracks {
        if let Some(existing) = track.language.as_ref().and_then(|language| {
            out.iter_mut()
                .find(|t| t.language.as_ref() == Some(language))
        }) {
            if track.default {
                *existing = track;
            }
        } else if out.len() < 64 {
            out.push(track);
        }
    }
    out
}

static ATTRIBUTE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"([A-Z0-9-]+)=(?:"([^"]*)"|([^,]*))(?:,|$)"#).expect("constant HLS regex")
});

pub fn hls_tracks(raw: &str) -> Result<Vec<AudioTrack>> {
    if !raw
        .trim_start_matches('\u{feff}')
        .trim_start()
        .starts_with("#EXTM3U")
    {
        bail!("Stream did not return an HLS playlist");
    }
    let tracks = raw
        .lines()
        .filter_map(|line| line.trim().strip_prefix("#EXT-X-MEDIA:"))
        .filter_map(|attributes| {
            let attrs: HashMap<_, _> = ATTRIBUTE_RE
                .captures_iter(attributes)
                .map(|c| {
                    (
                        c[1].to_string(),
                        c.get(2)
                            .or_else(|| c.get(3))
                            .map_or("", |v| v.as_str())
                            .to_string(),
                    )
                })
                .collect();
            (attrs.get("TYPE").map(String::as_str) == Some("AUDIO")).then_some(attrs)
        })
        .enumerate()
        .map(|(index, attrs)| {
            AudioTrack::new(
                attrs.get("LANGUAGE").map_or("", String::as_str),
                attrs.get("NAME").map_or("", String::as_str),
                index,
                attrs.get("DEFAULT").is_some_and(|value| value == "YES"),
            )
        });
    Ok(languages(tracks))
}

fn ffprobe_tracks(raw: &[u8]) -> Result<Vec<AudioTrack>> {
    let value: Value = serde_json::from_slice(raw).context("Invalid ffprobe audio metadata")?;
    let streams = value["streams"]
        .as_array()
        .context("ffprobe returned no stream metadata")?;
    Ok(languages(streams.iter().enumerate().map(
        |(index, stream)| {
            AudioTrack::new(
                stream["tags"]["language"].as_str().unwrap_or_default(),
                stream["tags"]["title"].as_str().unwrap_or_default(),
                index,
                stream["disposition"]["default"] == 1,
            )
        },
    )))
}

pub async fn probe(media: &Media) -> Result<Vec<AudioTrack>> {
    if let Media::Remote(stream) = media {
        let path = stream.url.path().to_ascii_lowercase();
        if path.ends_with(".m3u8") || path.contains("/playlist/") {
            let http = Http::with_timeouts(Duration::from_secs(5), Duration::from_secs(3))?;
            let (_, raw) = http
                .text_at(
                    http.client
                        .get(stream.url.clone())
                        .header("referer", &stream.referer),
                )
                .await?;
            return hls_tracks(&raw);
        }
    }
    let raw = probe_command(ffprobe(media, TRACK_ARGS)).await?;
    ffprobe_tracks(&raw)
}

/// Track-selection entries shared by [`ffprobe`] callers.
const TRACK_ARGS: &[&str] = &[
    "-select_streams",
    "a",
    "-show_entries",
    "stream_tags=language,title:stream_disposition=default",
];
/// Container duration entry used by [`duration`].
const DURATION_ARGS: &[&str] = &["-show_entries", "format=duration"];

/// Base ffprobe invocation for a media source: shared protocol whitelist,
/// headers, and JSON output. Callers append their entry selectors.
fn ffprobe(media: &Media, entries: &[&str]) -> Command {
    let mut command = Command::new("ffprobe");
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .args([
            "-v",
            "error",
            "-analyzeduration",
            "2000000",
            "-probesize",
            "1048576",
            "-of",
            "json",
            "-protocol_whitelist",
        ]);
    match media {
        Media::Remote(stream) => {
            command.arg("http,https,tcp,tls,crypto,data").args([
                "-user_agent",
                crate::provider::http::USER_AGENT,
                "-referer",
                &stream.referer,
            ]);
        }
        Media::Local(_) => {
            command.arg("file,crypto,data");
        }
    }
    command.args(entries).arg("-i").arg(media.source());
    command
}

/// Total media duration in seconds when ffprobe can determine it. Optional
/// metadata (e.g. for download-size estimates); callers may ignore errors.
pub async fn duration(media: &Media) -> Result<f64> {
    let raw = probe_command(ffprobe(media, DURATION_ARGS)).await?;
    duration_from_json(&raw)
}

fn duration_from_json(raw: &[u8]) -> Result<f64> {
    let value: Value = serde_json::from_slice(raw).context("Invalid ffprobe duration metadata")?;
    let seconds = value["format"]["duration"]
        .as_str()
        .and_then(|d| d.parse::<f64>().ok())
        .filter(|d| d.is_finite() && *d > 0.0)
        .context("ffprobe reported no usable duration")?;
    Ok(seconds)
}

async fn probe_command(mut command: Command) -> Result<Vec<u8>> {
    tokio::time::timeout(Duration::from_secs(8), async move {
        let mut child = command
            .spawn()
            .context("ffprobe is not installed or could not start")?;
        let stderr = child.stderr.take();
        let work = async {
            let mut raw = Vec::new();
            child
                .stdout
                .take()
                .context("ffprobe stdout unavailable")?
                .take(64 * 1024 + 1)
                .read_to_end(&mut raw)
                .await?;
            if raw.len() > 64 * 1024 {
                bail!("Audio metadata exceeded the safety limit");
            }
            Ok::<_, anyhow::Error>((child.wait().await?, raw))
        };
        let (result, output) = match stderr {
            Some(stderr) => media_error::capture(stderr, work).await,
            None => (work.await, media_error::Output::default()),
        };
        let (status, raw) = result?;
        if !status.success() {
            let error = output.failure.unwrap_or(MediaError {
                kind: DiagnosticFailure::Data,
                status: None,
            });
            error.record(DiagnosticOperation::AudioProbe);
            return Err(error.into());
        }
        Ok(raw)
    })
    .await
    .map_err(|_| {
        let error = MediaError {
            kind: DiagnosticFailure::Timeout,
            status: None,
        };
        error.record(DiagnosticOperation::AudioProbe);
        anyhow::Error::new(error)
    })?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hls_languages_ignore_subtitles_and_duplicate_quality_groups() -> Result<()> {
        let raw = "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"Italian, stereo\",DEFAULT=YES,LANGUAGE=\"it\"\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"English\",LANGUAGE=\"eng\"\n#EXT-X-MEDIA:TYPE=SUBTITLES,NAME=\"French\",LANGUAGE=\"fr\"\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"b\",NAME=\"English HQ\",LANGUAGE=\"en\"\n";
        let tracks = hls_tracks(raw)?;
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].label, "Italian, stereo");
        assert_eq!(tracks[0].language.as_deref(), Some("ita"));
        assert!(tracks[0].default);
        assert_eq!(tracks[1].language.as_deref(), Some("eng"));
        assert_eq!(tracks[1].index, 1);
        Ok(())
    }

    #[test]
    fn hls_handles_single_muxed_audio_and_unknown_languages() -> Result<()> {
        assert!(hls_tracks("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=100\nvideo.m3u8")?.is_empty());
        let tracks = hls_tracks(
            "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,NAME=\"Original\"\n#EXT-X-MEDIA:TYPE=AUDIO,LANGUAGE=\"und\"",
        )?;
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[1].label, "Audio track 2");
        assert!(tracks.iter().all(|t| t.language.is_none()));
        assert!(hls_tracks("<html>blocked</html>").is_err());
        Ok(())
    }

    #[test]
    fn file_metadata_uses_audio_ordinals_and_cleans_provider_text() -> Result<()> {
        let tracks = ffprobe_tracks(br#"{"streams":[{"tags":{"language":"ja","title":"Japanese\u001b"}},{"tags":{"language":"en-US"},"disposition":{"default":1}}]}"#)?;
        assert_eq!(tracks[0].language.as_deref(), Some("jpn"));
        assert_eq!(tracks[0].label, "Japanese");
        assert_eq!(tracks[1].language.as_deref(), Some("eng"));
        assert_eq!(tracks[1].label, "English");
        assert_eq!(tracks[1].index, 1);
        assert!(tracks[1].default);
        assert!(ffprobe_tracks(b"{}").is_err());
        assert_eq!(language_code("eng,ita"), None);
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failed_probe_reports_safe_http_codes_and_drains_stderr() -> Result<()> {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf '%s\\n' 'https://private.example/?token=SECRET: Server returned 503 Service Unavailable' >&2; exit 1"])
            .kill_on_drop(true).stdout(Stdio::piped()).stderr(Stdio::piped());
        let error = probe_command(command).await.expect_err("failed probe");
        assert_eq!(
            error
                .downcast_ref::<MediaError>()
                .expect("typed media error")
                .status,
            Some(503)
        );
        assert!(!format!("{error:#}").contains("SECRET"));
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "i=0; while [ \"$i\" -lt 10000 ]; do echo noise >&2; i=$((i + 1)); done; printf '%s' '{\"streams\":[]}'"])
            .kill_on_drop(true).stdout(Stdio::piped()).stderr(Stdio::piped());
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(3), probe_command(command))
                .await??
                .as_slice(),
            b"{\"streams\":[]}"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn duration_parses_container_metadata_and_rejects_junk() -> Result<()> {
        assert_eq!(
            duration_from_json(br#"{"format":{"duration":"1420.500000"}}"#)?,
            1420.5
        );
        for junk in [
            br#"{}"#.as_slice(),
            br#"{"format":{"duration":"0"}}"#.as_slice(),
            br#"{"format":{"duration":"not-a-number"}}"#.as_slice(),
            br#"{"format":{"duration":1420.5}}"#.as_slice(),
        ] {
            assert!(
                duration_from_json(junk).is_err(),
                "{:?}",
                String::from_utf8_lossy(junk)
            );
        }
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelling_a_probe_kills_its_subprocess() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let pid_file = directory.path().join("probe.pid");
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "echo $$ > \"$1\"; exec /bin/sleep 30", "probe-test"])
            .arg(&pid_file)
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let task = tokio::spawn(probe_command(command));
        let pid = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(value) = tokio::fs::read_to_string(&pid_file).await
                    && let Ok(pid) = value.trim().parse::<u32>()
                {
                    break pid;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        task.abort();
        assert!(task.await.expect_err("cancelled task").is_cancelled());
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let alive = std::process::Command::new("kill")
                    .args(["-0", &pid.to_string()])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .expect("kill probe")
                    .success();
                if !alive {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .context("cancelled probe left a running child")?;
        Ok(())
    }

    #[tokio::test]
    async fn remote_hls_probe_reads_only_the_master_without_ffprobe() -> Result<()> {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("probe connection");
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .expect("timeout");
            let mut raw = [0; 4096];
            let count = socket.read(&mut raw).expect("request");
            let request = String::from_utf8_lossy(&raw[..count]);
            assert!(
                request
                    .to_lowercase()
                    .contains("referer: https://player.example/")
            );
            let body = "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,LANGUAGE=\"ita\",NAME=\"Italian\"\n#EXT-X-MEDIA:TYPE=AUDIO,LANGUAGE=\"eng\",NAME=\"English\"\n";
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .expect("response");
        });
        let media = Media::Remote(crate::provider::Stream {
            url: format!("http://{address}/playlist/42").parse()?,
            referer: "https://player.example/".into(),
        });
        assert_eq!(probe(&media).await?.len(), 2);
        server.join().expect("server");
        Ok(())
    }
}
