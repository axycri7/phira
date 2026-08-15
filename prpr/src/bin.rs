//! Binary serialization and deserialization for prpr data structures.
//! Currently:
//!   - [crate::core::Chart]
//!   - [crate::core::ChartSettings]
//!   - [crate::core::JudgeLine]
//!   - [crate::core::Note]
//!   - [crate::core::Object]
//!   - [crate::core::CtrlObject]
//!   - [crate::core::Anim]
//!   - [crate::core::Keyframe]
//!   - [macroquad::prelude::Color]

use crate::{
    core::{
        Anim, AnimVector, BezierTween, BpmList, Chart, ChartExtra, ChartSettings, ClampedTween, CtrlObject, JudgeLine, JudgeLineCache, JudgeLineKind,
        Keyframe, Note, NoteKind, Object, StaticTween, Tweenable, TWEEN_FUNCTIONS, UIElement,
    },
    judge::{HitSound, JudgeStatus},
    parse::process_lines,
};
use anyhow::{bail, Context, Result};
use byteorder::{LittleEndian as LE, ReadBytesExt, WriteBytesExt};
use macroquad::{
    prelude::{Color, WHITE},
    texture::Texture2D,
};
use std::{
    cell::RefCell,
    collections::HashMap,
    io::{Read, Write},
    ops::Deref,
    rc::Rc,
};

pub trait BinaryData: Sized {
    fn read_binary<R: Read>(r: &mut BinaryReader<R>) -> Result<Self>;
    fn write_binary<W: Write>(&self, w: &mut BinaryWriter<W>) -> Result<()>;
}

const PBC_VERSION: u8 = 2;

pub struct BinaryReader<R: Read> {
    pub inner: R,
    time: u32,
    version: u8,
}

impl<R: Read> BinaryReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            inner: reader,
            time: 0,
            version: 0,
        }
    }

    pub fn reset_time(&mut self) {
        self.time = 0;
    }

    pub fn version(&self) -> u8 {
        self.version
    }

    pub fn set_version(&mut self, version: u8) {
        self.version = version;
    }

    pub fn time(&mut self) -> Result<f32> {
        self.time += self.uleb()? as u32;
        Ok(self.time as f32 / 1000.)
    }

    pub fn array<T: BinaryData>(&mut self) -> Result<Vec<T>> {
        (0..self.uleb()?).map(|_| self.read()).collect()
    }

    pub fn read<T: BinaryData>(&mut self) -> Result<T> {
        T::read_binary(self)
    }

    pub fn uleb(&mut self) -> Result<u64> {
        let mut result = 0;
        let mut shift = 0;
        loop {
            let byte = self.read::<u8>()?;
            result |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                break Ok(result);
            }
            shift += 7;
        }
    }
}

pub struct BinaryWriter<W: Write> {
    pub inner: W,
    time: u32,
}

impl<W: Write> BinaryWriter<W> {
    pub fn new(writer: W) -> Self {
        Self { inner: writer, time: 0 }
    }

    pub fn reset_time(&mut self) {
        self.time = 0;
    }

    pub fn time(&mut self, v: f32) -> Result<()> {
        let millis = (v * 1000.).round();
        if !millis.is_finite() || millis < 0. || millis > u32::MAX as f32 {
            bail!("invalid PBC timestamp: {v}s");
        }
        let v = millis as u32;
        if v < self.time {
            bail!("non-monotonic PBC timestamp: previous={}ms, next={}ms", self.time, v);
        }
        self.uleb((v - self.time) as _)?;
        self.time = v;
        Ok(())
    }

    pub fn array<T: BinaryData>(&mut self, v: &[T]) -> Result<()> {
        self.uleb(v.len() as _)?;
        for (index, element) in v.iter().enumerate() {
            element
                .write_binary(self)
                .with_context(|| format!("failed to write PBC array element #{index}"))?;
        }
        Ok(())
    }

    #[inline]
    pub fn write<T: BinaryData>(&mut self, v: &T) -> Result<()> {
        v.write_binary(self)
    }

    #[inline]
    pub fn write_val<T: BinaryData>(&mut self, v: T) -> Result<()> {
        v.write_binary(self)
    }

    pub fn uleb(&mut self, mut v: u64) -> Result<()> {
        loop {
            let mut byte = (v & 0x7f) as u8;
            v >>= 7;
            if v != 0 {
                byte |= 0x80;
            }
            self.write_val(byte)?;
            if v == 0 {
                break Ok(());
            }
        }
    }
}

impl BinaryData for u8 {
    fn read_binary<R: Read>(r: &mut BinaryReader<R>) -> Result<Self> {
        Ok(r.inner.read_u8()?)
    }

    fn write_binary<W: Write>(&self, w: &mut BinaryWriter<W>) -> Result<()> {
        Ok(w.inner.write_u8(*self)?)
    }
}

impl BinaryData for i32 {
    fn read_binary<R: Read>(r: &mut BinaryReader<R>) -> Result<Self> {
        Ok(r.inner.read_i32::<LE>()?)
    }

    fn write_binary<W: Write>(&self, w: &mut BinaryWriter<W>) -> Result<()> {
        Ok(w.inner.write_i32::<LE>(*self)?)
    }
}

impl BinaryData for bool {
    fn read_binary<R: Read>(r: &mut BinaryReader<R>) -> Result<Self> {
        Ok(r.inner.read_u8()? == 1)
    }

    fn write_binary<W: Write>(&self, w: &mut BinaryWriter<W>) -> Result<()> {
        Ok(w.inner.write_u8(if *self { 1 } else { 0 })?)
    }
}

impl BinaryData for f32 {
    fn read_binary<R: Read>(r: &mut BinaryReader<R>) -> Result<Self> {
        Ok(r.inner.read_f32::<LE>()?)
    }

    fn write_binary<W: Write>(&self, w: &mut BinaryWriter<W>) -> Result<()> {
        Ok(w.inner.write_f32::<LE>(*self)?)
    }
}

impl BinaryData for String {
    fn read_binary<R: Read>(r: &mut BinaryReader<R>) -> Result<Self> {
        Ok(String::from_utf8(r.array()?)?)
    }

    fn write_binary<W: Write>(&self, w: &mut BinaryWriter<W>) -> Result<()> {
        w.array(self.as_bytes())
    }
}

impl<T: BinaryData> BinaryData for Option<T> {
    fn read_binary<R: Read>(r: &mut BinaryReader<R>) -> Result<Self> {
        Ok(if r.read()? { Some(r.read()?) } else { None })
    }

    fn write_binary<W: Write>(&self, w: &mut BinaryWriter<W>) -> Result<()> {
        w.write_val(self.is_some())?;
        if let Some(value) = self {
            w.write(value)?;
        }
        Ok(())
    }
}

impl BinaryData for Color {
    fn read_binary<R: Read>(r: &mut BinaryReader<R>) -> Result<Self> {
        Ok(Self::from_rgba(r.read()?, r.read()?, r.read()?, r.read()?))
    }

    fn write_binary<W: Write>(&self, w: &mut BinaryWriter<W>) -> Result<()> {
        w.write_val((self.r * 256.) as u8)?;
        w.write_val((self.g * 256.) as u8)?;
        w.write_val((self.b * 256.) as u8)?;
        w.write_val((self.a * 256.) as u8)?;
        Ok(())
    }
}

impl BinaryData for HitSound {
    fn read_binary<R: Read>(r: &mut BinaryReader<R>) -> Result<Self> {
        Ok(match r.read::<u8>()? {
            0 => Self::None,
            1 => Self::Click,
            2 => Self::Flick,
            3 => Self::Drag,
            4 => Self::Custom(r.read()?),
            _ => bail!("invalid hitsound"),
        })
    }

    fn write_binary<W: Write>(&self, w: &mut BinaryWriter<W>) -> Result<()> {
        match self {
            Self::None => w.write_val(0_u8),
            Self::Click => w.write_val(1_u8),
            Self::Flick => w.write_val(2_u8),
            Self::Drag => w.write_val(3_u8),
            Self::Custom(name) => {
                w.write_val(4_u8)?;
                w.write(name)
            }
        }
    }
}

// IMPLEMENTATIONS

impl<T: BinaryData> BinaryData for Keyframe<T> {
    fn read_binary<R: Read>(r: &mut BinaryReader<R>) -> Result<Self> {
        Ok(Self {
            time: r.time()? as f64,
            value: r.read()?,
            tween: {
                let b = r.read::<u8>()?;
                match b & 0xC0 {
                    0 if (b as usize) < TWEEN_FUNCTIONS.len() => StaticTween::get_rc(b),
                    0 => bail!("invalid static tween id: {b}"),
                    0x80 if ((b & 0x7f) as usize) < TWEEN_FUNCTIONS.len() => Rc::new(ClampedTween::new(b & 0x7f, r.read()?..r.read()?)),
                    0x80 => bail!("invalid clamped tween id: {}", b & 0x7f),
                    0xC0 => Rc::new(BezierTween::new((r.read()?, r.read()?), (r.read()?, r.read()?))),
                    _ => bail!("invalid tween tag: {b:#04x}"),
                }
            },
        })
    }

    fn write_binary<W: Write>(&self, w: &mut BinaryWriter<W>) -> Result<()> {
        w.time(self.time as f32)?;
        w.write(&self.value)?;
        let tween = self.tween.as_any();
        if let Some(t) = tween.downcast_ref::<StaticTween>() {
            w.write_val(t.0)?;
        } else if let Some(t) = tween.downcast_ref::<ClampedTween>() {
            w.write_val(0x80 | t.0)?;
            w.write_val(t.1.start)?;
            w.write_val(t.1.end)?;
        } else if let Some(t) = tween.downcast_ref::<BezierTween>() {
            w.write_val(0xC0)?;
            w.write_val(t.p1.0)?;
            w.write_val(t.p1.1)?;
            w.write_val(t.p2.0)?;
            w.write_val(t.p2.1)?;
        } else {
            bail!("unsupported tween type in PBC keyframe");
        }
        Ok(())
    }
}

fn read_opt<R: Read, T: BinaryData + Tweenable>(r: &mut BinaryReader<R>) -> Result<Option<Box<Anim<T>>>> {
    Ok(match r.read::<u8>()? {
        0 => None,
        x => {
            let mut res = if x == 1 {
                Anim::default()
            } else {
                r.reset_time();
                Anim::new(r.array()?)
            };
            res.next = read_opt(r)?;
            Some(Box::new(res))
        }
    })
}

impl<T: BinaryData + Tweenable> BinaryData for Anim<T> {
    fn read_binary<R: Read>(r: &mut BinaryReader<R>) -> Result<Self> {
        Ok(*read_opt(r)?.unwrap())
    }

    fn write_binary<W: Write>(&self, w: &mut BinaryWriter<W>) -> Result<()> {
        let mut cur = self;
        loop {
            if cur.keyframes.is_empty() {
                w.write_val(1_u8)?;
            } else {
                w.write_val(2_u8)?;
                w.uleb(cur.keyframes.len() as _)?;
                w.reset_time();
                for (index, kf) in cur.keyframes.iter().enumerate() {
                    kf.write_binary(w).with_context(|| {
                        format!("failed to write keyframe #{index} at {:.3}s", kf.time)
                    })?;
                }
            }
            if let Some(next) = &cur.next {
                cur = next;
            } else {
                w.write_val(0_u8)?;
                break Ok(());
            }
        }
    }
}

impl BinaryData for Object {
    fn read_binary<R: Read>(r: &mut BinaryReader<R>) -> Result<Self> {
        Ok(Self {
            alpha: r.read()?,
            scale: AnimVector(r.read()?, r.read()?),
            rotation: r.read()?,
            translation: AnimVector(r.read()?, r.read()?),
        })
    }

    fn write_binary<W: Write>(&self, w: &mut BinaryWriter<W>) -> Result<()> {
        w.write(&self.alpha)?;
        w.write(&self.scale.0)?;
        w.write(&self.scale.1)?;
        w.write(&self.rotation)?;
        w.write(&self.translation.0)?;
        w.write(&self.translation.1)?;
        Ok(())
    }
}

impl BinaryData for CtrlObject {
    fn read_binary<R: Read>(r: &mut BinaryReader<R>) -> Result<Self> {
        assert_eq!(r.read::<u8>()?, 8);
        Ok(Self {
            alpha: r.read()?,
            size: r.read()?,
            pos: r.read()?,
            y: r.read()?,
        })
    }

    fn write_binary<W: Write>(&self, w: &mut BinaryWriter<W>) -> Result<()> {
        w.write_val(8_u8)?;
        w.write(&self.alpha)?;
        w.write(&self.size)?;
        w.write(&self.pos)?;
        w.write(&self.y)?;
        Ok(())
    }
}

impl BinaryData for Note {
    fn read_binary<R: Read>(r: &mut BinaryReader<R>) -> Result<Self> {
        let object = r.read()?;
        let kind = match r.read::<u8>()? {
            0 => NoteKind::Click,
            1 => NoteKind::Hold {
                end_time: r.read::<f32>()? as f64,
                end_height: r.read::<f32>()? as f64,
            },
            2 => NoteKind::Flick,
            3 => NoteKind::Drag,
            _ => bail!("invalid note kind"),
        };
        let time = r.time()? as f64;
        let height = r.read::<f32>()? as f64;
        let speed = if r.read()? { r.read::<f32>()? as f64 } else { 1. };
        let above = r.read()?;
        let fake = r.read()?;
        let (hitsound, color, fx_color, judge_area) = if r.version() >= 1 {
            (r.read()?, r.read()?, r.read()?, r.read()?)
        } else {
            (HitSound::default_from_kind(&kind), WHITE, None, 1.)
        };
        Ok(Self {
            object,
            kind,
            hitsound,
            time,
            height,
            speed,
            above,
            multiple_hint: false,
            fake,
            judge: JudgeStatus::NotJudged,
            color,
            fx_color,
            judge_area,
        })
    }

    fn write_binary<W: Write>(&self, w: &mut BinaryWriter<W>) -> Result<()> {
        w.write(&self.object)?;
        match self.kind {
            NoteKind::Click => {
                w.write_val(0_u8)?;
            }
            NoteKind::Hold { end_time, end_height } => {
                w.write_val(1_u8)?;
                w.write_val(end_time as f32)?;
                w.write_val(end_height as f32)?;
            }
            NoteKind::Flick => w.write_val(2_u8)?,
            NoteKind::Drag => w.write_val(3_u8)?,
        }
        w.time(self.time as f32)
            .with_context(|| format!("failed to write note timestamp at {:.3}s", self.time))?;
        w.write_val(self.height as f32)?;
        if self.speed == 1.0 {
            w.write_val(false)?;
        } else {
            w.write_val(true)?;
            w.write_val(self.speed as f32)?;
        }
        w.write_val(self.above)?;
        w.write_val(self.fake)?;
        w.write(&self.hitsound)?;
        w.write(&self.color)?;
        w.write(&self.fx_color)?;
        w.write(&self.judge_area)?;
        Ok(())
    }
}

impl BinaryData for JudgeLine {
    fn read_binary<R: Read>(r: &mut BinaryReader<R>) -> Result<Self> {
        r.reset_time();
        let object = r.read()?;
        let kind = match r.read::<u8>()? {
            0 => JudgeLineKind::Normal,
            1 => JudgeLineKind::Texture(Texture2D::empty().into(), r.read()?),
            2 => JudgeLineKind::Text(r.read()?),
            3 => JudgeLineKind::Paint(r.read()?, RefCell::default()),
            4 => unimplemented!(),
            _ => bail!("invalid judge line kind"),
        };
        let height = r.read()?;
        if r.version() >= 2 {
            r.reset_time();
        }
        let mut notes = r.array()?;
        let color = r.read()?;
        let parent = match r.uleb()? {
            0 => None,
            x => Some(x as usize - 1),
        };
        let flags = r.read::<u8>()?;
        let show_below = flags & 1 != 0;
        let rot_with_parent = flags & 2 != 0;
        let cache = JudgeLineCache::new(&mut notes);
        let attach_ui = UIElement::from_u8(r.read()?);
        let ctrl_obj = RefCell::new(r.read()?);
        let incline = r.read()?;
        let z_index = r.read()?;
        Ok(Self {
            object,
            kind,
            height,
            notes,
            color,
            parent,
            rot_with_parent,
            show_below,

            attach_ui,
            ctrl_obj,
            incline,
            z_index,

            cache,
        })
    }

    fn write_binary<W: Write>(&self, w: &mut BinaryWriter<W>) -> Result<()> {
        w.write(&self.object)?;
        match &self.kind {
            JudgeLineKind::Normal => w.write_val(0_u8)?,
            JudgeLineKind::Texture(_, path) => {
                w.write_val(1_u8)?;
                w.write(path)?;
            }
            JudgeLineKind::Text(text) => {
                w.write_val(2_u8)?;
                w.write(text)?;
            }
            JudgeLineKind::Paint(events, _) => {
                w.write_val(3_u8)?;
                w.write(events)?;
            }
            JudgeLineKind::TextureGif(..) => {
                bail!("gif texture binary not supported");
            }
        }
        w.write(&self.height)?;
        w.reset_time();
        w.array(&self.notes).context("failed to write judge line notes")?;
        w.write(&self.color)?;
        w.uleb(match self.parent {
            None => 0,
            Some(index) => index as u64 + 1,
        })?;
        w.write_val(self.show_below as u8 + self.rot_with_parent as u8 * 2)?;
        w.write_val(self.attach_ui.map_or(0, |it| it as u8))?;
        w.write(self.ctrl_obj.borrow().deref())?;
        w.write(&self.incline)?;
        w.write(&self.z_index)?;
        Ok(())
    }
}

impl BinaryData for ChartSettings {
    fn read_binary<R: Read>(r: &mut BinaryReader<R>) -> Result<Self> {
        Ok(Self {
            pe_alpha_extension: r.read::<u8>()? == 1,
            hold_partial_cover: r.read::<u8>()? == 1,
        })
    }

    fn write_binary<W: Write>(&self, w: &mut BinaryWriter<W>) -> Result<()> {
        w.write_val(self.pe_alpha_extension as u8)?;
        w.write_val(self.hold_partial_cover as u8)?;
        Ok(())
    }
}

impl BinaryData for Chart {
    fn read_binary<R: Read>(r: &mut BinaryReader<R>) -> Result<Self> {
        let mut offset: f32 = r.read()?;
        if offset.is_nan() {
            let version = r.read()?;
            r.set_version(version);
            offset = r.read()?;
        } else {
            r.set_version(0);
        }
        let mut lines = r.array()?;
        process_lines(&mut lines);
        let settings = r.read()?;
        Ok(Chart::new(offset, lines, BpmList::new(vec![(0., 60.)]), settings, ChartExtra::default(), HashMap::new()))
    }

    fn write_binary<W: Write>(&self, w: &mut BinaryWriter<W>) -> Result<()> {
        w.write_val(f32::NAN)?;
        w.write_val(PBC_VERSION)?;
        w.write_val(self.offset)?;
        w.array(&self.lines)?;
        w.write(&self.settings)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn assert_color_eq(actual: Color, expected: Color) {
        assert!((actual.r - expected.r).abs() <= 1.0 / 255.0);
        assert!((actual.g - expected.g).abs() <= 1.0 / 255.0);
        assert!((actual.b - expected.b).abs() <= 1.0 / 255.0);
        assert!((actual.a - expected.a).abs() <= 1.0 / 255.0);
    }

    fn assert_hitsound_eq(actual: &HitSound, expected: &HitSound) {
        match (actual, expected) {
            (HitSound::None, HitSound::None)
            | (HitSound::Click, HitSound::Click)
            | (HitSound::Flick, HitSound::Flick)
            | (HitSound::Drag, HitSound::Drag) => {}
            (HitSound::Custom(actual), HitSound::Custom(expected)) => assert_eq!(actual, expected),
            _ => panic!("hitsound mismatch"),
        }
    }

    fn note(time: f64, hitsound: HitSound) -> Note {
        Note {
            object: Object::default(),
            kind: NoteKind::Click,
            hitsound,
            time,
            height: 3.5,
            speed: 1.25,
            color: Color::new(0.25, 0.5, 0.75, 1.0),
            fx_color: Some(Color::new(0.75, 0.5, 0.25, 1.0)),
            judge_area: 1.5,
            above: true,
            multiple_hint: false,
            fake: false,
            judge: JudgeStatus::NotJudged,
        }
    }

    fn line(mut notes: Vec<Note>, parent: Option<usize>, attach_ui: Option<UIElement>) -> JudgeLine {
        let cache = JudgeLineCache::new(&mut notes);
        JudgeLine {
            object: Object::default(),
            kind: JudgeLineKind::Normal,
            height: Anim::fixed(2.0),
            notes,
            color: Anim::fixed(WHITE),
            parent,
            rot_with_parent: true,
            show_below: true,
            attach_ui,
            ctrl_obj: RefCell::new(CtrlObject::default()),
            incline: Anim::fixed(10.0),
            z_index: 7,
            cache,
        }
    }

    #[test]
    fn pbc_note_roundtrip_preserves_extended_metadata() {
        let note = Note {
            object: Object::default(),
            kind: NoteKind::Hold {
                end_time: 4.5,
                end_height: 8.25,
            },
            hitsound: HitSound::Custom("kick.wav".to_owned()),
            time: 1.25,
            height: 3.5,
            speed: 1.75,
            color: Color::new(0.25, 0.5, 0.75, 0.875),
            fx_color: Some(Color::new(0.875, 0.125, 0.375, 0.625)),
            judge_area: 1.5,
            above: false,
            multiple_hint: true,
            fake: true,
            judge: JudgeStatus::NotJudged,
        };

        let mut bytes = Vec::new();
        BinaryWriter::new(&mut bytes).write(&note).unwrap();

        let mut reader = BinaryReader::new(Cursor::new(bytes));
        reader.set_version(PBC_VERSION);
        let decoded: Note = reader.read().unwrap();

        match &decoded.kind {
            NoteKind::Hold { end_time, end_height } => {
                assert_eq!(*end_time, 4.5);
                assert_eq!(*end_height, 8.25);
            }
            _ => panic!("expected hold note"),
        }
        assert_hitsound_eq(&decoded.hitsound, &note.hitsound);
        assert_eq!(decoded.time, note.time);
        assert_eq!(decoded.height, note.height);
        assert_eq!(decoded.speed, note.speed);
        assert_eq!(decoded.above, note.above);
        assert_eq!(decoded.fake, note.fake);
        assert_color_eq(decoded.color, note.color);
        assert_color_eq(decoded.fx_color.unwrap(), note.fx_color.unwrap());
        assert_eq!(decoded.judge_area, note.judge_area);
        assert!(!decoded.multiple_hint);
        assert!(matches!(decoded.judge, JudgeStatus::NotJudged));
    }

    #[test]
    fn pbc_chart_roundtrip_preserves_line_and_note_metadata() {
        let chart = Chart::new(
            0.125,
            vec![
                line(vec![note(1.0, HitSound::Click)], None, Some(UIElement::Pause)),
                line(vec![note(1.0, HitSound::Custom("snare.wav".to_owned()))], Some(0), None),
            ],
            BpmList::new(vec![(0.0, 60.0)]),
            ChartSettings {
                pe_alpha_extension: true,
                hold_partial_cover: true,
            },
            ChartExtra::default(),
            HashMap::new(),
        );

        let mut bytes = Vec::new();
        BinaryWriter::new(&mut bytes).write(&chart).unwrap();
        let decoded: Chart = BinaryReader::new(Cursor::new(bytes)).read().unwrap();

        assert_eq!(decoded.offset, chart.offset);
        assert_eq!(decoded.lines.len(), 2);
        assert!(decoded.settings.pe_alpha_extension);
        assert!(decoded.settings.hold_partial_cover);
        assert!(matches!(decoded.lines[0].attach_ui, Some(UIElement::Pause)));
        assert_eq!(decoded.lines[1].parent, Some(0));
        assert!(decoded.lines[1].rot_with_parent);
        assert!(decoded.lines[1].show_below);
        assert_eq!(decoded.lines[1].z_index, 7);

        let decoded_note = &decoded.lines[1].notes[0];
        assert_hitsound_eq(&decoded_note.hitsound, &HitSound::Custom("snare.wav".to_owned()));
        assert_color_eq(decoded_note.color, Color::new(0.25, 0.5, 0.75, 1.0));
        assert_color_eq(decoded_note.fx_color.unwrap(), Color::new(0.75, 0.5, 0.25, 1.0));
        assert_eq!(decoded_note.judge_area, 1.5);
        assert!(decoded.lines[0].notes[0].multiple_hint);
        assert!(decoded.lines[1].notes[0].multiple_hint);
    }

    #[test]
    fn pbc_chart_roundtrip_allows_notes_before_line_animation_end() {
        let mut judge_line = line(vec![note(1.0, HitSound::Click)], None, None);
        judge_line.object.translation.1 = Anim::new(vec![
            Keyframe::new(0.0, 0.0, 0),
            Keyframe::new(5.0, 100.0, 0),
        ]);
        let chart = Chart::new(
            0.0,
            vec![judge_line],
            BpmList::new(vec![(0.0, 60.0)]),
            ChartSettings::default(),
            ChartExtra::default(),
            HashMap::new(),
        );

        let mut bytes = Vec::new();
        BinaryWriter::new(&mut bytes).write(&chart).unwrap();
        let decoded: Chart = BinaryReader::new(Cursor::new(bytes)).read().unwrap();

        assert_eq!(decoded.lines[0].notes[0].time, 1.0);
    }
}
