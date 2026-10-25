//! WebVTT subtitles: parsing and finding the cue to show.

use std::collections::HashMap;
use std::time::Duration;

use super::model::SubtitleTrack;

/// A piece of subtitle text shown from `start` until `end`.
#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    pub start: Duration,
    pub end: Duration,
    pub text: String,
    pub placement: Placement,
}

/// Where a cue is shown, from its WebVTT cue settings. The default shows
/// it centered at the bottom.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Placement {
    /// `line`: the vertical position.
    pub line: Line,
    /// The edge of the cue at a [`Line::Percent`].
    pub line_align: Anchor,
    /// `position`: the horizontal anchor in percent of the width; derived
    /// from `align` when `None`.
    pub position: Option<f64>,
    /// The edge of the cue at `position`; derived from `align` when `None`.
    pub position_align: Option<Anchor>,
    /// `size`: the width of the cue box in percent; 100 when `None`.
    pub size: Option<f64>,
    /// `align`: the alignment of the text in the cue box.
    pub align: Align,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Line {
    /// The last line, as for `Number(-1)`.
    #[default]
    Auto,
    /// A line counted from the top from 0, or from the bottom from -1.
    Number(i32),
    /// Percent of the height.
    Percent(f64),
}

/// An edge or the center of a cue: top, center or bottom for a line;
/// left, center or right for a position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Anchor {
    #[default]
    Start,
    Center,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    Start,
    #[default]
    Center,
    End,
    Left,
    Right,
}

/// The horizontal alignment of text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

/// A cue's box, resolved from its [`Placement`] for left-to-right text,
/// with horizontal values as fractions of the video width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CueBox {
    pub left: f64,
    pub width: f64,
    pub align: TextAlign,
    pub vertical: Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Vertical {
    /// A line counted from the top from 0, or from the bottom from -1.
    Line(i32),
    /// The fraction of the video height where the cue's `Anchor` is.
    At(f64, Anchor),
}

impl Placement {
    /// The cue box, as WebVTT's rendering rules compute it.
    pub fn cue_box(&self) -> CueBox {
        let position = self.position.unwrap_or(match self.align {
            Align::Left => 0.0,
            Align::Right => 100.0,
            _ => 50.0,
        });
        let position_align = self.position_align.unwrap_or(match self.align {
            Align::Start | Align::Left => Anchor::Start,
            Align::End | Align::Right => Anchor::End,
            Align::Center => Anchor::Center,
        });
        let max_size = match position_align {
            Anchor::Start => 100.0 - position,
            Anchor::End => position,
            Anchor::Center => 2.0 * position.min(100.0 - position),
        };
        let size = self.size.unwrap_or(100.0).min(max_size);
        let left = match position_align {
            Anchor::Start => position,
            Anchor::End => position - size,
            Anchor::Center => position - size / 2.0,
        };
        CueBox {
            left: left / 100.0,
            width: size / 100.0,
            align: match self.align {
                Align::Start | Align::Left => TextAlign::Left,
                Align::Center => TextAlign::Center,
                Align::End | Align::Right => TextAlign::Right,
            },
            vertical: match self.line {
                Line::Auto => Vertical::Line(-1),
                Line::Number(n) => Vertical::Line(n),
                Line::Percent(p) => Vertical::At(p / 100.0, self.line_align),
            },
        }
    }
}

/// Parses cues of WebVTT or of YouTube's srv3 format.
pub fn parse(subtitles: &str) -> Vec<Cue> {
    if subtitles.trim_start().starts_with('<') {
        parse_srv3(subtitles)
    } else {
        parse_vtt(subtitles)
    }
}

/// A srv3 window position (`<wp>`): the anchor point of the window, 0 to 8
/// row by row from the top left, at `h` and `v` percent of the video.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Window {
    anchor: u8,
    h: f64,
    v: f64,
}

/// Centered at the bottom.
const DEFAULT_WINDOW: Window = Window {
    anchor: 7,
    h: 50.0,
    v: 100.0,
};

/// Parses cues of YouTube's srv3 format (`<timedtext format="3">`), placed
/// by their window positions and styles. Markup is removed.
pub fn parse_srv3(xml: &str) -> Vec<Cue> {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_str(xml);
    let mut windows = HashMap::new();
    let mut aligns = HashMap::new();
    let mut cues = Vec::new();
    // The open paragraph: its timing, placement and escaped text.
    let mut paragraph: Option<(Duration, Duration, Placement, String)> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) => {
                let attr = |name| {
                    let value = e.try_get_attribute(name).ok()??;
                    let value = value
                        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .ok()?;
                    Some(value.into_owned())
                };
                let number = |name| attr(name).and_then(|v| v.parse::<f64>().ok());
                match e.name().as_ref() {
                    "wp" => {
                        let window = Window {
                            anchor: attr("ap")
                                .and_then(|a| a.parse().ok())
                                .filter(|a| *a <= 8)
                                .unwrap_or(DEFAULT_WINDOW.anchor),
                            h: number("ah").unwrap_or(DEFAULT_WINDOW.h),
                            v: number("av").unwrap_or(DEFAULT_WINDOW.v),
                        };
                        windows.extend(attr("id").map(|id| (id, window)));
                    }
                    "ws" => {
                        let align = match attr("ju").as_deref() {
                            Some("0") => Align::Left,
                            Some("1") => Align::Right,
                            _ => Align::Center,
                        };
                        aligns.extend(attr("id").map(|id| (id, align)));
                    }
                    "p" => {
                        let window = attr("wp").and_then(|id| windows.get(&id).copied());
                        let align = attr("ws").and_then(|id| aligns.get(&id).copied());
                        let placement = window_placement(
                            window.unwrap_or(DEFAULT_WINDOW),
                            align.unwrap_or(Align::Center),
                        );
                        let millis = |name| number(name).map(|n| Duration::from_millis(n as u64));
                        paragraph = millis("t").map(|start| {
                            let end = start + millis("d").unwrap_or_default();
                            (start, end, placement, String::new())
                        });
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(e)) => {
                if let Some((.., text)) = &mut paragraph {
                    text.push_str(&e.html_content());
                }
            }
            Ok(Event::GeneralRef(e)) => {
                if let Some((.., text)) = &mut paragraph {
                    text.push_str(&format!("&{};", e.html_content()));
                }
            }
            Ok(Event::End(e)) if e.name().as_ref() == "p" => {
                let Some((start, end, placement, text)) = paragraph.take() else {
                    continue;
                };
                let text = quick_xml::escape::unescape(&text)
                    .map(|t| t.into_owned())
                    .unwrap_or(text);
                let text = text
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() {
                    cues.push(Cue {
                        start,
                        end,
                        text,
                        placement,
                    });
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    cues
}

/// The placement of a cue in `window` with text aligned by `align`.
fn window_placement(window: Window, align: Align) -> Placement {
    if window == DEFAULT_WINDOW && align == Align::Center {
        return Placement::default();
    }
    let anchor = |i: u8| [Anchor::Start, Anchor::Center, Anchor::End][usize::from(i)];
    // YouTube keeps windows 2% away from the edges.
    let percent = |p: f64| 2.0 + p.clamp(0.0, 100.0) * 0.96;
    Placement {
        line: Line::Percent(percent(window.v)),
        line_align: anchor(window.anchor / 3),
        position: Some(percent(window.h)),
        position_align: Some(anchor(window.anchor % 3)),
        size: None,
        align,
    }
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
    let mut rest = rest.split_whitespace();
    let end = rest.next()?;
    let placement = placement(rest);
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
        placement,
    })
}

/// The placement from cue settings such as `line:10%` or `align:left`.
/// Invalid settings are ignored.
fn placement<'a>(settings: impl Iterator<Item = &'a str>) -> Placement {
    let mut placement = Placement::default();
    for setting in settings {
        let Some((name, value)) = setting.split_once(':') else {
            continue;
        };
        let (value, anchor) = match value.split_once(',') {
            Some((value, anchor)) => (value, Some(anchor)),
            None => (value, None),
        };
        match name {
            "line" => {
                let line = match value.strip_suffix('%') {
                    Some(value) => percent(value).map(Line::Percent),
                    None => value.parse().ok().map(Line::Number),
                };
                let line_align = match anchor {
                    None | Some("start") => Some(Anchor::Start),
                    Some("center") => Some(Anchor::Center),
                    Some("end") => Some(Anchor::End),
                    Some(_) => None,
                };
                if let (Some(line), Some(line_align)) = (line, line_align) {
                    placement.line = line;
                    placement.line_align = line_align;
                }
            }
            "position" => {
                let position_align = match anchor {
                    None | Some("auto") => Some(None),
                    Some("line-left") => Some(Some(Anchor::Start)),
                    Some("center") => Some(Some(Anchor::Center)),
                    Some("line-right") => Some(Some(Anchor::End)),
                    Some(_) => None,
                };
                if let (Some(position), Some(position_align)) =
                    (value.strip_suffix('%').and_then(percent), position_align)
                {
                    placement.position = Some(position);
                    placement.position_align = position_align;
                }
            }
            "size" if anchor.is_none() => {
                if let Some(size) = value.strip_suffix('%').and_then(percent) {
                    placement.size = Some(size);
                }
            }
            "align" if anchor.is_none() => {
                let align = match value {
                    "start" => Align::Start,
                    "center" => Align::Center,
                    "end" => Align::End,
                    "left" => Align::Left,
                    "right" => Align::Right,
                    _ => continue,
                };
                placement.align = align;
            }
            _ => {}
        }
    }
    placement
}

/// Parses a percentage without its `%`, such as `12.5`, from 0 to 100.
fn percent(s: &str) -> Option<f64> {
    let valid = s.starts_with(|c: char| c.is_ascii_digit())
        && s.chars().all(|c| c.is_ascii_digit() || c == '.');
    s.parse()
        .ok()
        .filter(|p| valid && (0.0..=100.0).contains(p))
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

/// Writes `cues` as WebVTT, so that [`parse_vtt`] reads them back.
pub fn to_vtt(cues: &[Cue]) -> String {
    let timestamp = |t: Duration| {
        let secs = t.as_secs();
        format!(
            "{:02}:{:02}:{:02}.{:03}",
            secs / 3600,
            secs / 60 % 60,
            secs % 60,
            t.subsec_millis()
        )
    };
    let mut vtt = String::from("WEBVTT\n");
    for cue in cues {
        let text = cue
            .text
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        vtt += &format!(
            "\n{} --> {}{}\n{text}\n",
            timestamp(cue.start),
            timestamp(cue.end),
            settings(&cue.placement)
        );
    }
    vtt
}

/// The cue settings of `placement`, each preceded by a space.
fn settings(placement: &Placement) -> String {
    let anchor = |anchor| match anchor {
        Anchor::Start => "start",
        Anchor::Center => "center",
        Anchor::End => "end",
    };
    let mut settings = String::new();
    match placement.line {
        Line::Auto => {}
        Line::Number(n) => settings += &format!(" line:{n}"),
        Line::Percent(p) => {
            settings += &format!(" line:{p}%,{}", anchor(placement.line_align));
        }
    }
    if let Some(position) = placement.position {
        let align = match placement.position_align {
            None => "auto",
            Some(Anchor::Start) => "line-left",
            Some(Anchor::Center) => "center",
            Some(Anchor::End) => "line-right",
        };
        settings += &format!(" position:{position}%,{align}");
    }
    if let Some(size) = placement.size {
        settings += &format!(" size:{size}%");
    }
    let align = match placement.align {
        Align::Start => "start",
        Align::Center => "center",
        Align::End => "end",
        Align::Left => "left",
        Align::Right => "right",
    };
    if placement.align != Align::Center {
        settings += &format!(" align:{align}");
    }
    settings
}

/// The text to show centered at the bottom at `position`: all active
/// cues with the default [`Placement`], one per line.
pub fn text_at(cues: &[Cue], position: Duration) -> Option<String> {
    let active: Vec<_> = active_at(cues, position)
        .filter(|c| c.placement == Placement::default())
        .map(|c| c.text.as_str())
        .collect();
    (!active.is_empty()).then(|| active.join("\n"))
}

/// The active cues at `position` that are placed by cue settings.
pub fn placed_at(cues: &[Cue], position: Duration) -> Vec<&Cue> {
    active_at(cues, position)
        .filter(|c| c.placement != Placement::default())
        .collect()
}

fn active_at(cues: &[Cue], position: Duration) -> impl Iterator<Item = &Cue> {
    cues.iter()
        .filter(move |c| c.start <= position && position < c.end)
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
            placement: Placement::default(),
        }
    }

    fn settings(settings: &str) -> Placement {
        let vtt = format!("WEBVTT\n\n00:01.000 --> 00:02.000 {settings}\nHi\n");
        parse_vtt(&vtt)[0].placement
    }

    #[test]
    fn parses_cue_settings() {
        assert_eq!(
            settings("align:left size:35% position:0%"),
            Placement {
                align: Align::Left,
                size: Some(35.0),
                position: Some(0.0),
                ..Placement::default()
            }
        );
        assert_eq!(
            settings("line:10% position:12.5%,line-right align:end"),
            Placement {
                line: Line::Percent(10.0),
                position: Some(12.5),
                position_align: Some(Anchor::End),
                align: Align::End,
                ..Placement::default()
            }
        );
        assert_eq!(
            settings("line:50%,center"),
            Placement {
                line: Line::Percent(50.0),
                line_align: Anchor::Center,
                ..Placement::default()
            }
        );
        assert_eq!(
            settings("line:-2 align:start"),
            Placement {
                line: Line::Number(-2),
                align: Align::Start,
                ..Placement::default()
            }
        );
        assert_eq!(settings("line:3,end").line, Line::Number(3));
    }

    #[test]
    fn ignores_invalid_and_unsupported_settings() {
        for invalid in [
            "align:middle",
            "size:120%",
            "size:35",
            "position:-5%",
            "position:5%,top",
            "line:1.5",
            "line:%",
            "line:10%,left",
            "vertical:rl",
            "region:fred",
            "nonsense",
        ] {
            assert_eq!(settings(invalid), Placement::default(), "{invalid}");
        }
    }

    fn cue_box(settings_: &str) -> CueBox {
        settings(settings_).cue_box()
    }

    #[test]
    fn default_cue_box_is_centered_at_the_bottom() {
        assert_eq!(
            cue_box(""),
            CueBox {
                left: 0.0,
                width: 1.0,
                align: TextAlign::Center,
                vertical: Vertical::Line(-1),
            }
        );
    }

    #[test]
    fn cue_box_follows_position_size_and_align() {
        let boxed = |settings| {
            let b = cue_box(settings);
            (b.left, b.width, b.align)
        };
        assert_eq!(
            boxed("align:left size:35% position:0%"),
            (0.0, 0.35, TextAlign::Left)
        );
        assert_eq!(
            boxed("align:right size:35% position:100%"),
            (0.65, 0.35, TextAlign::Right)
        );
        // Without a position, start and end text is anchored at the center.
        assert_eq!(boxed("align:start"), (0.5, 0.5, TextAlign::Left));
        assert_eq!(boxed("align:end"), (0.0, 0.5, TextAlign::Right));
        assert_eq!(boxed("align:right"), (0.0, 1.0, TextAlign::Right));
        // The box is limited to the video.
        assert_eq!(boxed("position:30%"), (0.0, 0.6, TextAlign::Center));
        assert_eq!(
            boxed("position:80% size:30%"),
            (0.65, 0.3, TextAlign::Center)
        );
        assert_eq!(
            boxed("position:40%,line-right size:50% align:left"),
            (0.0, 0.4, TextAlign::Left)
        );
    }

    #[test]
    fn cue_box_follows_line() {
        let vertical = |settings| cue_box(settings).vertical;
        assert_eq!(vertical("line:-2"), Vertical::Line(-2));
        assert_eq!(vertical("line:0"), Vertical::Line(0));
        assert_eq!(vertical("line:10%"), Vertical::At(0.1, Anchor::Start));
        assert_eq!(
            vertical("line:50%,center"),
            Vertical::At(0.5, Anchor::Center)
        );
    }

    fn srv3(head: &str, body: &str) -> Vec<Cue> {
        parse_srv3(&format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\" ?><timedtext format=\"3\">\n\
             <head>{head}</head>\n<body>{body}</body>\n</timedtext>"
        ))
    }

    #[test]
    fn parses_srv3() {
        let cues = parse_srv3(include_str!("../../tests/fixtures/subtitles_srv3.xml"));
        assert_eq!(cues.len(), 2);
        assert_eq!((cues[0].start, cues[0].end), (ms(2_000), ms(3_000)));
        assert_eq!(cues[0].text, "ezrab_\u{200b}: FREEDOM");
        assert_eq!(
            cues[1].text.lines().last(),
            Some("haze_dev\u{200b}: I'm FREEEE")
        );
        assert_eq!(
            cues[1].placement,
            Placement {
                line: Line::Percent(2.0),
                line_align: Anchor::Start,
                position: Some(2.0 + 62.0 * 0.96),
                position_align: Some(Anchor::Start),
                size: None,
                align: Align::Left,
            }
        );
    }

    #[test]
    fn srv3_without_windows_is_unplaced() {
        let cues = srv3(
            "<wp id=\"0\"/><ws id=\"0\"/>",
            "<p t=\"500\" d=\"1000\">Tom &amp; Jerry\n<s p=\"1\">&lt;3</s></p>\
             <p t=\"2000\" d=\"1000\" wp=\"0\" ws=\"0\">Hi</p>\
             <p t=\"3000\" d=\"1000\">\n</p>",
        );
        assert_eq!(
            cues,
            [
                Cue {
                    start: ms(500),
                    end: ms(1_500),
                    text: "Tom & Jerry\n<3".into(),
                    placement: Placement::default(),
                },
                cue(2_000, 3_000, "Hi"),
            ]
        );
    }

    #[test]
    fn srv3_windows_place_cues() {
        let cues = srv3(
            "<wp id=\"1\" ap=\"8\" ah=\"100\" av=\"100\"/>\
             <wp id=\"2\" ap=\"4\" ah=\"50\" av=\"50\"/>\
             <ws id=\"1\" ju=\"1\"/><ws id=\"2\" ju=\"2\"/>",
            "<p t=\"0\" d=\"1\" wp=\"1\" ws=\"1\">a</p>\
             <p t=\"0\" d=\"1\" wp=\"2\" ws=\"2\">b</p>\
             <p t=\"0\" d=\"1\" ws=\"1\">c</p>",
        );
        let placements: Vec<_> = cues.iter().map(|c| c.placement).collect();
        assert_eq!(
            placements,
            [
                Placement {
                    line: Line::Percent(98.0),
                    line_align: Anchor::End,
                    position: Some(98.0),
                    position_align: Some(Anchor::End),
                    size: None,
                    align: Align::Right,
                },
                Placement {
                    line: Line::Percent(50.0),
                    line_align: Anchor::Center,
                    position: Some(50.0),
                    position_align: Some(Anchor::Center),
                    size: None,
                    align: Align::Center,
                },
                Placement {
                    line: Line::Percent(98.0),
                    line_align: Anchor::End,
                    position: Some(50.0),
                    position_align: Some(Anchor::Center),
                    size: None,
                    align: Align::Right,
                },
            ]
        );
    }

    #[test]
    fn parse_detects_the_format() {
        let vtt = "WEBVTT\n\n00:01.000 --> 00:02.000\nHi\n";
        assert_eq!(parse(vtt), parse_vtt(vtt));
        let xml = include_str!("../../tests/fixtures/subtitles_srv3.xml");
        assert_eq!(parse(xml), parse_srv3(xml));
        assert_eq!(parse(xml).len(), 2);
    }

    #[test]
    fn parses_positioned_captions() {
        let cues = parse_vtt(include_str!(
            "../../tests/fixtures/subtitles_positioned.vtt"
        ));
        let placements: Vec<_> = cues.iter().map(|c| c.placement).collect();
        assert_eq!(
            placements[0],
            Placement {
                align: Align::Right,
                size: Some(35.0),
                position: Some(100.0),
                ..Placement::default()
            }
        );
        assert_eq!(placements[3].line, Line::Percent(10.0));
        assert_eq!(
            cues[3].text,
            "how to change the text colour\nand positioning of captions."
        );
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
    fn accepts_short_timestamps_and_crlf() {
        let vtt = "WEBVTT\r\n\r\n00:05.000 --> 00:06.250\r\nHi\r\n";
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
    fn writes_vtt_that_parses_back() {
        let cues = [
            cue(1_360, 3_040, "Tom & Jerry <3"),
            cue(3_723_000, 3_724_500, "Two\nlines"),
            Cue {
                placement: settings("line:-2 position:12.5%,line-right size:35% align:left"),
                ..cue(5_000, 6_000, "Placed")
            },
            Cue {
                placement: settings("line:10%,end"),
                ..cue(6_000, 7_000, "Placed")
            },
        ];
        let vtt = to_vtt(&cues);
        assert!(
            vtt.starts_with("WEBVTT\n\n00:00:01.360 --> 00:00:03.040\n"),
            "{vtt}"
        );
        assert_eq!(parse_vtt(&vtt), cues);
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

    #[test]
    fn placed_cues_are_shown_apart() {
        let top = Cue {
            placement: settings("line:0"),
            ..cue(0, 2_000, "top")
        };
        let cues = [cue(0, 2_000, "a"), top.clone(), cue(1_000, 3_000, "b")];
        assert_eq!(text_at(&cues, ms(1_500)).as_deref(), Some("a\nb"));
        assert_eq!(placed_at(&cues, ms(1_500)), [&top]);
        assert_eq!(text_at(std::slice::from_ref(&top), ms(500)), None);
        assert!(placed_at(&cues, ms(2_500)).is_empty());
    }
}
