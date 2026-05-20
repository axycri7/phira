//! Chart parsers
use anyhow::Result;
use std::io::Cursor;

use crate::{bin::BinaryReader, core::Chart, fs::FileSystem, info::ChartFormat};

prpr_l10n::tl_file!("parser" ptl);

mod extra;
pub use extra::parse_extra;

mod pec;
pub use pec::parse_pec;

mod pgr;
pub use pgr::parse_phigros;

mod rpe;
pub use rpe::{lint, parse_rpe, RPE_HEIGHT, RPE_WIDTH};

#[derive(Debug, Default)]
pub struct ParseWarnings {
    pub has_new_speed_events: bool,
    pub has_attach_ui: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ParseOptions {
    pub use_rpe_170_speed: bool,
}

pub async fn parse_chart_bytes(
    bytes: &[u8],
    format: ChartFormat,
    fs: &mut dyn FileSystem,
    extra: crate::core::ChartExtra,
    options: ParseOptions,
) -> Result<Chart> {
    tracing::trace!(?format, bytes = bytes.len(), "parse chart bytes");
    match format {
        ChartFormat::Rpe => {
            let source = String::from_utf8_lossy(bytes);
            parse_rpe(&source, fs, extra, options.use_rpe_170_speed).await
        }
        ChartFormat::Pgr => {
            let source = String::from_utf8_lossy(bytes);
            parse_phigros(&source, extra)
        }
        ChartFormat::Pec => {
            let source = String::from_utf8_lossy(bytes);
            parse_pec(&source, extra)
        }
        ChartFormat::Pbc => {
            let mut r = BinaryReader::new(Cursor::new(bytes));
            r.read()
        }
    }
}

pub fn infer_chart_format_bytes(explicit: Option<&ChartFormat>, bytes: &[u8]) -> ChartFormat {
    explicit.cloned().unwrap_or_else(|| {
        if let Ok(text) = std::str::from_utf8(bytes) {
            if text.starts_with('{') {
                if text.contains("\"META\"") {
                    ChartFormat::Rpe
                } else {
                    ChartFormat::Pgr
                }
            } else {
                ChartFormat::Pec
            }
        } else {
            ChartFormat::Pbc
        }
    })
}

pub(crate) fn process_lines(v: &mut [crate::core::JudgeLine]) {
    use crate::ext::NotNanExt;
    use std::collections::HashMap;

    let total_notes = v.iter().map(|line| line.notes.len()).sum();
    let mut counts = HashMap::with_capacity(total_notes);
    for note in v.iter().flat_map(|line| line.notes.iter()) {
        let count = counts.entry(note.time.not_nan()).or_insert(0_u8);
        *count = 2_u8.min(*count + 1);
    }
    for note in v.iter_mut().flat_map(|line| line.notes.iter_mut()) {
        note.multiple_hint = counts.get(&note.time.not_nan()).copied().unwrap_or_default() > 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        core::{Anim, ChartExtra, CtrlObject, JudgeLine, JudgeLineCache, JudgeLineKind, Note, NoteKind, Object},
        judge::{HitSound, JudgeStatus},
    };
    use macroquad::prelude::WHITE;
    use std::cell::RefCell;

    fn note(time: f64) -> Note {
        Note {
            object: Object::default(),
            kind: NoteKind::Click,
            hitsound: HitSound::Click,
            time,
            height: 0.0,
            speed: 1.0,
            color: WHITE,
            fx_color: None,
            judge_area: 1.0,
            above: true,
            multiple_hint: false,
            fake: false,
            judge: JudgeStatus::NotJudged,
        }
    }

    fn line(mut notes: Vec<Note>) -> JudgeLine {
        let cache = JudgeLineCache::new(&mut notes);
        JudgeLine {
            object: Object::default(),
            ctrl_obj: RefCell::new(CtrlObject::default()),
            kind: JudgeLineKind::Normal,
            height: Default::default(),
            incline: Default::default(),
            notes,
            color: Anim::fixed(WHITE),
            parent: None,
            rot_with_parent: false,
            z_index: 0,
            show_below: false,
            attach_ui: None,
            cache,
        }
    }

    #[test]
    fn process_lines_marks_only_duplicate_note_times() {
        let mut lines = vec![line(vec![note(1.0), note(2.0)]), line(vec![note(2.0), note(3.0)])];

        process_lines(&mut lines);

        let hints = lines
            .iter()
            .flat_map(|line| line.notes.iter().map(|note| (note.time, note.multiple_hint)))
            .collect::<Vec<_>>();
        assert_eq!(hints, vec![(1.0, false), (2.0, true), (2.0, true), (3.0, false)]);
    }

    #[test]
    fn infer_chart_format_bytes_handles_text_binary_and_explicit_formats() {
        assert_eq!(infer_chart_format_bytes(None, br#"{"META":{}}"#), ChartFormat::Rpe);
        assert_eq!(infer_chart_format_bytes(None, br#"{"formatVersion":3}"#), ChartFormat::Pgr);
        assert_eq!(infer_chart_format_bytes(None, b"0 1 2 3"), ChartFormat::Pec);
        assert_eq!(infer_chart_format_bytes(None, &[0xff, 0x00]), ChartFormat::Pbc);
        assert_eq!(infer_chart_format_bytes(Some(&ChartFormat::Pbc), br#"{"META":{}}"#), ChartFormat::Pbc);
    }

    #[test]
    fn parse_small_pgr_fixture_preserves_note_ordering_and_hints() {
        let source = r#"{
            "formatVersion": 3,
            "offset": 0.125,
            "judgeLineList": [{
                "bpm": 120,
                "judgeLineDisappearEvents": [{"startTime": 0, "endTime": 1, "start": 1, "end": 1}],
                "judgeLineRotateEvents": [{"startTime": 0, "endTime": 1, "start": 0, "end": 0}],
                "judgeLineMoveEvents": [{"startTime": 0, "endTime": 1, "start": 0.5, "end": 0.5, "start2": 0.5, "end2": 0.5}],
                "speedEvents": [{"startTime": 0, "endTime": 1, "value": 1}],
                "notesAbove": [
                    {"type": 1, "time": 32, "positionX": 0, "holdTime": 0, "speed": 1, "floorPosition": 0}
                ],
                "notesBelow": [
                    {"type": 4, "time": 32, "positionX": 0, "holdTime": 0, "speed": 1, "floorPosition": 0}
                ]
            }]
        }"#;

        let chart = parse_phigros(source, ChartExtra::default()).unwrap();

        assert_eq!(chart.offset, 0.125);
        assert_eq!(chart.lines.len(), 1);
        assert_eq!(chart.lines[0].notes.len(), 2);
        assert!(chart.lines[0].notes.iter().all(|note| note.multiple_hint));
    }

    #[test]
    fn parse_small_pec_fixture_preserves_note_count_and_hints() {
        let source = "\
0
bp 0 120
cv 0 0 5.85
cp 0 0 1024 700
cd 0 0 0
ca 0 0 255
n1 0 0 512 1 0
n3 0 0 512 1 0
";

        let chart = parse_pec(source, ChartExtra::default()).unwrap();

        assert_eq!(chart.lines.len(), 1);
        assert_eq!(chart.lines[0].notes.len(), 2);
        assert!(chart.settings.pe_alpha_extension);
        assert!(chart.lines[0].notes.iter().all(|note| note.multiple_hint));
    }
}

#[rustfmt::skip]
pub const RPE_TWEEN_MAP: [crate::core::TweenId; 30] = {
    use crate::core::{easing_from as e, TweenMajor::*, TweenMinor::*};
    [
        2, 2, // linear
        e(Sine, Out), e(Sine, In),
        e(Quad, Out), e(Quad, In),
        e(Sine, InOut), e(Quad, InOut),
        e(Cubic, Out), e(Cubic, In),
        e(Quart, Out), e(Quart, In),
        e(Cubic, InOut), e(Quart, InOut),
        e(Quint, Out), e(Quint, In),
        e(Expo, Out), e(Expo, In),
        e(Circ, Out), e(Circ, In),
        e(Back, Out), e(Back, In),
        e(Circ, InOut), e(Back, InOut),
        e(Elastic, Out), e(Elastic, In),
        e(Bounce, Out), e(Bounce, In),
        e(Bounce, InOut), e(Elastic, InOut),
    ]
};
