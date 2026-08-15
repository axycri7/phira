use super::{BadNote, BpmList, Effect, JudgeLine, JudgeLineKind, Matrix, Resource, UIElement, Vector};
use crate::{core::Object, ext::SafeTexture, fs::FileSystem, judge::JudgeStatus, ui::Ui};
use anyhow::{Context, Result};
use macroquad::prelude::*;
use nalgebra::Rotation2;
use sasa::AudioClip;
use std::{cell::RefCell, collections::HashMap};

#[derive(Default)]
pub struct ChartExtra {
    pub effects: Vec<Effect>,
    pub global_effects: Vec<Effect>,
    #[cfg(feature = "video")]
    pub videos: Vec<(super::Video, Option<super::VideoAttach>)>,
}

#[derive(Default)]
pub struct ChartSettings {
    pub pe_alpha_extension: bool,
    pub hold_partial_cover: bool,
}

pub type HitSoundMap = HashMap<String, AudioClip>;

#[derive(Default)]
pub struct ChartFrameCache {
    pub time: f64,
    pub positions: Vec<Vector>,
    pub transforms: Vec<Matrix>,
    pub rotations: Vec<f32>,
    pub line_heights: Vec<f64>,
    states: Vec<u8>,
}

impl ChartFrameCache {
    fn resize(&mut self, len: usize) {
        self.positions.resize(len, Vector::default());
        self.transforms.resize(len, Matrix::identity());
        self.rotations.resize(len, 0.0);
        self.line_heights.resize(len, 0.0);
        self.states.resize(len, 0);
        self.states.fill(0);
    }
}

pub struct Chart {
    pub offset: f32,
    pub lines: Vec<JudgeLine>,
    pub bpm_list: RefCell<BpmList>,

    pub settings: ChartSettings,
    pub extra: ChartExtra,

    /// Line order according to z-index, lines with attach_ui will be removed from this list
    ///
    /// Store the index of the line in z-index ascending order
    pub order: Vec<usize>,
    /// TODO: docs from RPE
    pub attach_ui: [Option<usize>; 7],

    pub hitsounds: HitSoundMap,
    pub frame_cache: RefCell<ChartFrameCache>,
}

impl Chart {
    pub fn new(offset: f32, lines: Vec<JudgeLine>, bpm_list: BpmList, settings: ChartSettings, extra: ChartExtra, hitsounds: HitSoundMap) -> Self {
        let mut attach_ui = [None; 7];
        let mut order = (0..lines.len())
            .filter(|it| {
                if let Some(element) = lines[*it].attach_ui {
                    attach_ui[element as usize - 1] = Some(*it);
                    false
                } else {
                    true
                }
            })
            .collect::<Vec<_>>();
        order.sort_by_key(|it| (lines[*it].z_index, *it));
        Self {
            offset,
            lines,
            bpm_list: RefCell::new(bpm_list),
            settings,
            extra,

            order,
            attach_ui,

            hitsounds,
            frame_cache: RefCell::default(),
        }
    }

    pub fn prepare_frame_cache(&self, res: &Resource) {
        let mut cache = self.frame_cache.borrow_mut();
        if cache.time == res.time && cache.positions.len() == self.lines.len() {
            return;
        }
        cache.resize(self.lines.len());
        cache.time = res.time;
        for id in 0..self.lines.len() {
            prepare_line_frame_cache(id, &self.lines, res, &mut cache);
        }
    }

    #[inline]
    pub fn with_element<R>(
        &self,
        ui: &mut Ui,
        res: &Resource,
        element: UIElement,
        scale_point: Option<(f32, f32)>,
        rotation_point: (f32, f32),
        f: impl FnOnce(&mut Ui, Color) -> R,
    ) -> R {
        let scale_point = scale_point.unwrap_or(rotation_point);
        if let Some(id) = self.attach_ui[element as usize - 1] {
            let lines = &self.lines;
            let line = &lines[id];
            let obj = &line.object;
            let cache = self.frame_cache.borrow();
            let mut tr = if cache.time == res.time {
                cache.positions.get(id).copied().unwrap_or_else(|| line.fetch_pos(res, lines))
            } else {
                line.fetch_pos(res, lines)
            };
            drop(cache);
            tr.y = -tr.y;
            let color = self.lines[id].color.now_opt().unwrap_or(WHITE);
            let scale = obj.now_scale(Vector::new(scale_point.0, scale_point.1));
            let ro =
                Object::new_rotation_wrt_point(Rotation2::new(-obj.rotation.now().to_radians()), Vector::new(rotation_point.0, rotation_point.1));
            ui.with(Matrix::new_translation(&tr) * ro * scale, |ui| ui.alpha(obj.now_alpha().max(0.), |ui| f(ui, color)))
        } else {
            f(ui, WHITE)
        }
    }

    pub async fn load_textures(&mut self, fs: &mut dyn FileSystem) -> Result<()> {
        for line in &mut self.lines {
            if let JudgeLineKind::Texture(tex, path) = &mut line.kind {
                if tex.width() > 1. && tex.height() > 1. {
                    continue;
                }
                *tex = SafeTexture::from(
                    image::load_from_memory(&fs.load_file(path).await.with_context(|| format!("failed to load illustration {path}"))?)?,
                )
                .with_mipmap();
            }
        }
        Ok(())
    }

    pub fn reset(&mut self) {
        self.lines
            .iter_mut()
            .flat_map(|it| it.notes.iter_mut())
            .for_each(|note| note.judge = JudgeStatus::NotJudged);
        for line in &mut self.lines {
            line.cache.reset(&mut line.notes);
        }
        #[cfg(feature = "video")]
        for (video, _) in &mut self.extra.videos {
            if let Err(err) = video.reset() {
                use crate::parse::{ptl, L10N_LOCAL};
                crate::scene::show_error(err.context(ptl!("video-load-failed", "path" => video.video_file.path().to_string_lossy())));
            }
        }
    }

    pub fn update(&mut self, res: &mut Resource) {
        for line in &mut self.lines {
            line.object.set_time(res.time);
            line.height.set_time(res.time);
        }
        self.frame_cache.borrow_mut().time = f64::NAN;
        self.prepare_frame_cache(res);
        let cache = self.frame_cache.borrow();
        for (id, line) in self.lines.iter_mut().enumerate() {
            line.update(res, cache.transforms[id], cache.rotations[id], cache.line_heights[id]);
        }
        for effect in &mut self.extra.effects {
            effect.update(res);
        }
        #[cfg(feature = "video")]
        for (video, _) in &mut self.extra.videos {
            if let Err(err) = video.update(res.time) {
                tracing::warn!("video error: {err:?}");
            }
        }
    }

    fn note_screen_transform(flip_x: bool) -> Matrix {
        Matrix::identity().append_nonuniform_scaling(&Vector::new(if flip_x { -1. } else { 1. }, -1.))
    }

    pub fn render(&self, ui: &mut Ui, res: &mut Resource, bad_notes: &mut Vec<BadNote>) {
        res.note_buffer.borrow_mut().begin_frame();
        self.prepare_frame_cache(res);
        #[cfg(feature = "video")]
        for (video, attach) in &self.extra.videos {
            if let Some(attach) = attach {
                let line = &self.lines[attach.line];
                let color = line.color.now_opt().unwrap_or(res.judge_line_color);
                let mat = self.lines[attach.line].object.now(res);
                res.apply_model_of(&mat, |res| {
                    video.render(res.time, res.aspect_ratio, color);
                });
            } else {
                video.render(res.time, res.aspect_ratio, WHITE);
            }
        }
        res.apply_model_of(&Self::note_screen_transform(res.config.flip_x()), |res| {
            let mut guard = self.bpm_list.borrow_mut();
            let cache = self.frame_cache.borrow();
            for id in &self.order {
                self.lines[*id].render(ui, res, &mut guard, &self.settings, *id, cache.transforms[*id], cache.line_heights[*id]);
            }
            drop(guard);
            bad_notes.retain(|note| note.render(res));
            res.note_buffer.borrow_mut().flush();
            if res.config.sample_count > 1 {
                unsafe { get_internal_gl() }.flush();
                if let Some(target) = &res.chart_target {
                    target.blit();
                }
            }
        });
    }

    pub fn render_effects(&self, res: &mut Resource) {
        if res.no_effect {
            return;
        }
        res.apply_model_of(&Self::note_screen_transform(false), |res| {
            for effect in &self.extra.effects {
                effect.render(res);
            }
        });
    }
}

fn compose_line_transform(parent: Option<(Vector, f32)>, own_translation: Vector, own_rotation: f32, rot_with_parent: bool) -> (Vector, f32) {
    if let Some((parent_position, parent_rotation)) = parent {
        (
            parent_position + Rotation2::new(parent_rotation.to_radians()) * own_translation,
            own_rotation + if rot_with_parent { parent_rotation } else { 0. },
        )
    } else {
        (own_translation, own_rotation)
    }
}

fn prepare_line_frame_cache(id: usize, lines: &[JudgeLine], res: &Resource, cache: &mut ChartFrameCache) {
    if cache.states[id] == 2 {
        return;
    }
    if cache.states[id] == 1 {
        tracing::warn!(line = id, "cycle detected in judge line parent graph");
        return;
    }

    cache.states[id] = 1;
    let line = &lines[id];
    let parent = line.parent.filter(|parent| *parent < lines.len());
    let parent_ready = if let Some(parent) = parent {
        prepare_line_frame_cache(parent, lines, res, cache);
        cache.states[parent] == 2
    } else {
        false
    };

    let own_rotation = line.object.rotation.now();
    let own_translation = line.object.now_translation(res);
    let parent_transform = parent
        .filter(|_| parent_ready)
        .map(|parent| (cache.positions[parent], cache.rotations[parent]));
    let (position, rotation) = compose_line_transform(parent_transform, own_translation, own_rotation, line.rot_with_parent);

    cache.positions[id] = position;
    cache.rotations[id] = rotation;
    cache.transforms[id] = Rotation2::new(rotation.to_radians()).to_homogeneous().append_translation(&position);
    cache.line_heights[id] = line.height.now() as f64;
    cache.states[id] = 2;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Point;

    fn recursive_transform(
        id: usize,
        parents: &[Option<usize>],
        rotations: &[f32],
        translations: &[Vector],
        rot_with_parent: &[bool],
    ) -> (Vector, f32) {
        let parent = parents[id].map(|parent| recursive_transform(parent, parents, rotations, translations, rot_with_parent));
        compose_line_transform(parent, translations[id], rotations[id], rot_with_parent[id])
    }

    fn cached_transforms(parents: &[Option<usize>], rotations: &[f32], translations: &[Vector], rot_with_parent: &[bool]) -> Vec<(Vector, f32)> {
        let mut output = Vec::with_capacity(parents.len());
        for id in 0..parents.len() {
            let parent = parents[id].map(|parent| output[parent]);
            output.push(compose_line_transform(parent, translations[id], rotations[id], rot_with_parent[id]));
        }
        output
    }
    #[test]
    fn frame_cache_matches_recursive_parent_transforms() {
        let parents = [None, Some(0), Some(1), None];
        let rotations = [15., -35., 80., 5.];
        let translations = [
            Vector::new(0.2, -0.1),
            Vector::new(-0.3, 0.4),
            Vector::new(0.6, 0.25),
            Vector::new(-0.2, -0.5),
        ];
        let rot_with_parent = [false, true, false, false];
        let cached = cached_transforms(&parents, &rotations, &translations, &rot_with_parent);

        for id in 0..parents.len() {
            let expected = recursive_transform(id, &parents, &rotations, &translations, &rot_with_parent);
            assert!((cached[id].0 - expected.0).norm() <= 1e-6);
            assert!((cached[id].1 - expected.1).abs() <= 1e-6);
        }
    }

    #[test]
    fn note_screen_transform_keeps_notes_moving_toward_the_judge_line() {
        let point = Point::new(0.25, 0.5);

        assert_eq!(Chart::note_screen_transform(false).transform_point(&point), Point::new(0.25, -0.5));
        assert_eq!(Chart::note_screen_transform(true).transform_point(&point), Point::new(-0.25, -0.5));
    }
}
