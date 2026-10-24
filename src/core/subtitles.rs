//! WebVTT subtitles: parsing and finding the cue to show.

use std::time::Duration;

use super::model::SubtitleTrack;

/// A piece of subtitle text shown from `start` until `end`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cue {
    pub start: Duration,
    pub end: Duration,
    pub text: String,
}

/// Parses WebVTT cues. Markup such as `<c>` tags and inline timestamps is
/// removed; malformed blocks are skipped.
pub fn parse_vtt(vtt: &str) -> Vec<Cue> {
    let lines: Vec<_> = vtt.lines().collect();
    // Blocks end at empty lines; lines of only spaces belong to a cue.
    lines
        .split(|line| line.is_empty())
        .filter_map(parse_block)
        .collect()
}

fn parse_block(lines: &[&str]) -> Option<Cue> {
    let first = lines.first()?;
    if ["WEBVTT", "NOTE", "STYLE", "REGION"]
        .iter()
        .any(|keyword| first.starts_with(keyword))
    {
        return None;
    }
    // An optional identifier precedes the timing line.
    let timing = lines.iter().take(2).position(|l| l.contains("-->"))?;
    let (start, rest) = lines[timing].split_once("-->")?;
    let end = rest.split_whitespace().next()?;
    let text = lines[timing + 1..]
        .iter()
        .map(|line| plain_text(line))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then_some(Cue {
        start: timestamp(start.trim())?,
        end: timestamp(end)?,
        text,
    })
}

/// Parses `hh:mm:ss.ttt` or `mm:ss.ttt`.
fn timestamp(s: &str) -> Option<Duration> {
    let (rest, millis) = s.split_once('.')?;
    let mut secs = 0;
    for part in rest.split(':') {
        secs = secs * 60 + part.parse::<u64>().ok()?;
    }
    Some(Duration::from_secs(secs) + Duration::from_millis(millis.parse().ok()?))
}

/// A cue text line without tags and with entities decoded, trimmed.
fn plain_text(line: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for c in line.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            c if !in_tag => text.push(c),
            _ => {}
        }
    }
    let text = [
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&nbsp;", " "),
        ("&lrm;", ""),
        ("&rlm;", ""),
        ("&amp;", "&"),
    ]
    .iter()
    .fold(text, |text, (entity, c)| text.replace(entity, c));
    text.trim().to_string()
}

/// The text to show at `position`: all active cues, one per line.
pub fn text_at(cues: &[Cue], position: Duration) -> Option<String> {
    let active: Vec<_> = cues
        .iter()
        .filter(|c| c.start <= position && position < c.end)
        .map(|c| c.text.as_str())
        .collect();
    (!active.is_empty()).then(|| active.join("\n"))
}

/// The index of the track to show for the preferred language `lang`:
/// the same language, then the base language (`en` for `en-GB`), then
/// other variants of it (`en-US`); manual before auto-generated tracks.
pub fn preferred_track(tracks: &[SubtitleTrack], lang: &str) -> Option<usize> {
    let base = |lang: &str| lang.split('-').next().unwrap_or_default().to_lowercase();
    tracks
        .iter()
        .enumerate()
        .filter_map(|(index, track)| {
            let rank = if track.lang.eq_ignore_ascii_case(lang) {
                0
            } else if track.lang.eq_ignore_ascii_case(&base(lang)) {
                1
            } else if base(&track.lang) == base(lang) {
                2
            } else {
                return None;
            };
            Some((index, (rank, track.auto_generated)))
        })
        .min_by_key(|(_, key)| *key)
        .map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(lang: &str, auto_generated: bool) -> SubtitleTrack {
        SubtitleTrack {
            name: lang.into(),
            lang: lang.into(),
            url: String::new(),
            auto_generated,
        }
    }

    #[test]
    fn prefers_same_language_then_manual_tracks() {
        let tracks = [
            track("de", true),
            track("en", true),
            track("en-GB", false),
            track("en", false),
            track("pt-BR", true),
        ];
        assert_eq!(preferred_track(&tracks, "en"), Some(3));
        assert_eq!(preferred_track(&tracks, "EN-gb"), Some(2));
        assert_eq!(preferred_track(&tracks, "en-US"), Some(3));
        assert_eq!(preferred_track(&tracks, "de-DE"), Some(0));
        assert_eq!(preferred_track(&tracks, "pt"), Some(4));
        assert_eq!(preferred_track(&tracks, "fr"), None);
        assert_eq!(preferred_track(&[], "en"), None);
    }

    fn ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    fn cue(start: u64, end: u64, text: &str) -> Cue {
        Cue {
            start: ms(start),
            end: ms(end),
            text: text.into(),
        }
    }

    #[test]
    fn parses_cues() {
        let vtt = "WEBVTT\nKind: captions\nLanguage: en\n\n\
                   00:00:01.360 --> 00:00:03.040\n[♪♪♪]\n\n\
                   intro\n01:02:03.000 --> 01:02:04.500\nTwo\nlines\n";
        assert_eq!(
            parse_vtt(vtt),
            [
                cue(1_360, 3_040, "[♪♪♪]"),
                cue(3_723_000, 3_724_500, "Two\nlines"),
            ]
        );
    }

    #[test]
    fn accepts_short_timestamps_crlf_and_settings() {
        let vtt = "WEBVTT\r\n\r\n00:05.000 --> 00:06.250 align:start position:0%\r\nHi\r\n";
        assert_eq!(parse_vtt(vtt), [cue(5_000, 6_250, "Hi")]);
    }

    #[test]
    fn strips_markup_and_skips_other_blocks() {
        let vtt = "WEBVTT\n\nNOTE a comment\n\nSTYLE\n::cue { color: red }\n\n\
                   00:00:01.000 --> 00:00:02.000\n<v Bob>Tom &amp; <i>Jerry</i>&nbsp;&lt;3\n\n\
                   not a timestamp --> either\nskipped\n\n\
                   00:00:03.000 --> 00:00:04.000\n \n\n";
        assert_eq!(parse_vtt(vtt), [cue(1_000, 2_000, "Tom & Jerry <3")]);
    }

    #[test]
    fn parses_auto_generated_captions() {
        let cues = parse_vtt(include_str!("../../tests/fixtures/subtitles_asr.vtt"));
        let texts: Vec<_> = cues.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts[0], "[Music]");
        assert_eq!(texts[1], "We're no strangers to");
        assert_eq!(
            texts[3],
            "We're no strangers to\nlove. You know the rules and so do"
        );
        assert_eq!(cues[1].start, ms(18_800));
    }

    #[test]
    fn text_at_joins_active_cues() {
        let cues = [
            cue(0, 2_000, "a"),
            cue(1_000, 3_000, "b"),
            cue(5_000, 6_000, "c"),
        ];
        assert_eq!(text_at(&cues, ms(500)).as_deref(), Some("a"));
        assert_eq!(text_at(&cues, ms(1_500)).as_deref(), Some("a\nb"));
        assert_eq!(
            text_at(&cues, ms(2_000)).as_deref(),
            Some("b"),
            "end is exclusive"
        );
        assert_eq!(text_at(&cues, ms(4_000)), None);
        assert_eq!(text_at(&cues, ms(9_000)), None);
    }
}
