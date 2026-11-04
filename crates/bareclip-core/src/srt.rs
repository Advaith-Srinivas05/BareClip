//! Reads SRT subtitle files. These come from many programs, so anything that can be understood is
//! kept and anything else is passed over.

use crate::time::{TICKS_PER_SECOND, Ticks};

/// One caption: its words, and when it shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cue {
    pub start: Ticks,
    pub end: Ticks,
    pub text: String,
}

/// Turns a subtitle file's bytes into text. Older files are often not UTF-8, and each byte is then
/// taken as Windows' usual Western characters, which is right far more often than not.
pub fn decode_subtitles(bytes: &[u8]) -> String {
    let from_units = |units: Vec<u16>| String::from_utf16_lossy(&units);
    match bytes {
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        [0xFF, 0xFE, rest @ ..] => from_units(
            rest.as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair))
                .collect(),
        ),
        [0xFE, 0xFF, rest @ ..] => from_units(
            rest.as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_be_bytes(*pair))
                .collect(),
        ),
        _ => match std::str::from_utf8(bytes) {
            Ok(text) => text.to_owned(),
            Err(_) => bytes.iter().map(|byte| char::from(*byte)).collect(),
        },
    }
}

/// A time such as 00:01:02,500. The hours can be left out, and a full stop can stand for the comma.
fn parse_time(text: &str) -> Option<Ticks> {
    let text = text.trim().replace(',', ".");
    let mut parts = text.rsplit(':');
    let seconds: f64 = parts.next()?.trim().parse().ok()?;
    let minutes: f64 = parts.next()?.trim().parse().ok()?;
    let hours: f64 = match parts.next() {
        Some(hours) => hours.trim().parse().ok()?,
        None => 0.0,
    };
    if parts.next().is_some() || seconds < 0.0 || minutes < 0.0 || hours < 0.0 {
        return None;
    }
    let total = hours * 3600.0 + minutes * 60.0 + seconds;
    total
        .is_finite()
        .then(|| Ticks((total * TICKS_PER_SECOND as f64).round() as i64))
}

/// The start and end from a line such as `00:00:01,000 --> 00:00:02,500 X1:40`.
fn parse_timing(line: &str) -> Option<(Ticks, Ticks)> {
    let (start, end) = line.split_once("-->")?;
    let end = end.split_whitespace().next()?;
    Some((parse_time(start)?, parse_time(end)?))
}

/// Takes out styling such as `<i>` and `{\an8}`, which BareClip has no way to show.
fn without_styling(line: &str) -> String {
    let mut kept = String::with_capacity(line.len());
    let mut closing = None;
    for character in line.chars() {
        match closing {
            Some(end) => {
                if character == end {
                    closing = None;
                }
            }
            None => match character {
                '<' => closing = Some('>'),
                '{' => closing = Some('}'),
                other => kept.push(other),
            },
        }
    }
    kept.trim().to_owned()
}

/// Every caption in a file that has words and lasts some time, in order of when they start.
pub fn parse_srt(text: &str) -> Vec<Cue> {
    let mut cues: Vec<Cue> = Vec::new();
    let mut current: Option<(Ticks, Ticks, Vec<String>)> = None;
    let mut finish = |current: &mut Option<(Ticks, Ticks, Vec<String>)>| {
        if let Some((start, end, lines)) = current.take()
            && end > start
            && !lines.is_empty()
        {
            cues.push(Cue {
                start,
                end,
                text: lines.join("\n"),
            });
        }
    };
    for line in text.lines() {
        if let Some((start, end)) = parse_timing(line) {
            finish(&mut current);
            current = Some((start, end, Vec::new()));
        } else if line.trim().is_empty() {
            finish(&mut current);
        } else if let Some((_, _, lines)) = &mut current {
            let words = without_styling(line);
            if !words.is_empty() {
                lines.push(words);
            }
        }
    }
    finish(&mut current);
    cues.sort_by_key(|cue| cue.start);
    cues
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seconds(value: f64) -> Ticks {
        Ticks((value * TICKS_PER_SECOND as f64).round() as i64)
    }

    #[test]
    fn a_plain_file_gives_its_captions_with_their_times() {
        let cues = parse_srt(
            "1\n00:00:01,000 --> 00:00:02,500\nHello there\n\n2\n00:01:00,250 --> 01:00:00,000\nTwo\nlines\n",
        );
        assert_eq!(
            cues,
            vec![
                Cue {
                    start: seconds(1.0),
                    end: seconds(2.5),
                    text: "Hello there".to_owned()
                },
                Cue {
                    start: seconds(60.25),
                    end: seconds(3600.0),
                    text: "Two\nlines".to_owned()
                },
            ]
        );
    }

    #[test]
    fn windows_line_endings_and_full_stops_are_understood() {
        let cues = parse_srt("1\r\n00:00:01.000 --> 00:00:02.000\r\nHello\r\n\r\n");
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].text, "Hello");
        assert_eq!(cues[0].end, seconds(2.0));
    }

    #[test]
    fn numbers_can_be_missing_and_blank_lines_doubled() {
        let cues = parse_srt(
            "\n\n00:00:01,000 --> 00:00:02,000\nOne\n\n\n\n00:00:03,000 --> 00:00:04,000\nTwo",
        );
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[1].text, "Two");
    }

    #[test]
    fn hours_can_be_left_out() {
        let cues = parse_srt("01:30.500 --> 01:32.000\nShort times\n");
        assert_eq!(cues[0].start, seconds(90.5));
    }

    #[test]
    fn position_notes_after_a_time_are_ignored() {
        let cues = parse_srt("00:00:01,000 --> 00:00:02,000 X1:40 X2:600 Y1:20 Y2:50\nHi\n");
        assert_eq!(cues[0].end, seconds(2.0));
    }

    #[test]
    fn styling_is_taken_out_and_words_are_kept() {
        let cues = parse_srt(
            "00:00:01,000 --> 00:00:02,000\n{\\an8}<i>Slanted</i> and <font color=\"red\">red</font>\n",
        );
        assert_eq!(cues[0].text, "Slanted and red");
    }

    #[test]
    fn a_caption_with_no_words_or_no_length_is_passed_over() {
        let cues = parse_srt(
            "00:00:01,000 --> 00:00:02,000\n<i></i>\n\n00:00:05,000 --> 00:00:05,000\nNo length\n\n00:00:09,000 --> 00:00:08,000\nBackwards\n\n00:00:10,000 --> 00:00:11,000\nKept\n",
        );
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].text, "Kept");
    }

    #[test]
    fn a_broken_block_doesnt_lose_the_ones_around_it() {
        let cues = parse_srt(
            "1\n00:00:01,000 --> 00:00:02,000\nOne\n\n2\nnot a time --> at all\nLost\n\n3\n00:00:03,000 --> 00:00:04,000\nThree\n",
        );
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[1].text, "Three");
    }

    #[test]
    fn captions_come_out_in_order_of_time() {
        let cues = parse_srt(
            "00:00:05,000 --> 00:00:06,000\nLater\n\n00:00:01,000 --> 00:00:02,000\nFirst\n",
        );
        assert_eq!(cues[0].text, "First");
    }

    #[test]
    fn something_that_isnt_subtitles_gives_nothing() {
        assert!(parse_srt("").is_empty());
        assert!(parse_srt("Just some notes\nabout nothing\n").is_empty());
    }

    #[test]
    fn every_way_a_file_can_be_written_comes_out_as_the_same_words() {
        let words = "Caf\u{e9}";
        assert_eq!(decode_subtitles(words.as_bytes()), words);
        let with_mark = [&[0xEF, 0xBB, 0xBF], words.as_bytes()].concat();
        assert_eq!(decode_subtitles(&with_mark), words);
        let little: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain(words.encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        assert_eq!(decode_subtitles(&little), words);
        let big: Vec<u8> = [0xFE, 0xFF]
            .into_iter()
            .chain(words.encode_utf16().flat_map(u16::to_be_bytes))
            .collect();
        assert_eq!(decode_subtitles(&big), words);
        assert_eq!(decode_subtitles(b"Caf\xE9"), words, "an older Western file");
    }
}
