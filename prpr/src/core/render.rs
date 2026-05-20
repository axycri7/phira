use macroquad::{
    prelude::{DrawMode, Vertex},
    texture::{RenderTarget, Texture2D},
    window::get_internal_gl,
};
use miniquad::{gl::GLuint, FilterMode, RenderPass, Texture, TextureFormat};
use std::collections::BTreeMap;

use super::resource::MAX_SIZE;

// TODO: doc
pub struct MSRenderTarget {
    logical_dim: (u32, u32),
    texture_dim: (u32, u32),
    samples: u32,
    fbo: GLuint,
    rbo: GLuint,
    dummy: RenderTarget,
    output: [Option<RenderTarget>; 2],
}

pub fn copy_fbo(src: GLuint, dst: GLuint, dim: (u32, u32)) -> bool {
    unsafe {
        use miniquad::gl::*;
        glBindFramebuffer(GL_READ_FRAMEBUFFER, src);
        glBindFramebuffer(GL_DRAW_FRAMEBUFFER, dst);
        let (w, h) = (dim.0 as i32, dim.1 as i32);
        glBlitFramebuffer(0, 0, w, h, 0, 0, w, h, GL_COLOR_BUFFER_BIT, GL_NEAREST);
        glGetError() == GL_NO_ERROR
    }
}

pub fn internal_id(target: RenderTarget) -> GLuint {
    target.render_pass.gl_internal_id(unsafe { get_internal_gl() }.quad_context)
}

impl MSRenderTarget {
    pub fn new(dim: (u32, u32), samples: u32) -> Self {
        Self::new_scaled(dim, dim, samples)
    }

    pub fn new_scaled(logical_dim: (u32, u32), texture_dim: (u32, u32), samples: u32) -> Self {
        let texture_dim = (texture_dim.0.max(1), texture_dim.1.max(1));
        let mut fbo = 0;
        let mut rbo = 0;
        unsafe {
            use miniquad::gl::*;
            glGenRenderbuffers(1, &mut rbo as *mut _);
            glBindRenderbuffer(GL_RENDERBUFFER, rbo);
            glRenderbufferStorageMultisample(
                GL_RENDERBUFFER,
                samples as _,
                GL_RGB8,
                logical_dim.0 as _,
                logical_dim.1 as _,
            );
            glGenFramebuffers(1, &mut fbo as *mut _);
            glBindFramebuffer(GL_FRAMEBUFFER, fbo);
            glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_RENDERBUFFER, rbo);
        }
        let gl = unsafe { get_internal_gl() };
        let texture = Texture::new_render_texture(
            gl.quad_context,
            miniquad::TextureParams {
                width: texture_dim.0,
                height: texture_dim.1,
                format: TextureFormat::RGB8,
                filter: FilterMode::Linear,
                ..Default::default()
            },
        );
        let render_pass = RenderPass::new(gl.quad_context, texture, None);
        let dummy_render_pass = RenderPass::from_raw(gl.quad_context, fbo, texture);
        Self {
            logical_dim,
            texture_dim,
            samples,
            fbo,
            rbo,
            dummy: RenderTarget {
                texture: Texture2D::from_miniquad_texture(texture),
                render_pass: dummy_render_pass,
            },
            output: [
                Some(RenderTarget {
                    texture: Texture2D::from_miniquad_texture(texture),
                    render_pass,
                }),
                None,
            ],
        }
    }

    pub fn blit(&self) {
        copy_fbo(self.fbo, internal_id(self.output[0].unwrap()), self.logical_dim);
    }

    pub fn swap(&mut self) {
        self.output.swap(0, 1);
        if self.output[0].is_none() {
            let gl = unsafe { get_internal_gl() };
            let texture = miniquad::Texture::new_render_texture(
                gl.quad_context,
                miniquad::TextureParams {
                    width: self.texture_dim.0,
                    height: self.texture_dim.1,
                    format: TextureFormat::RGB8,
                    filter: FilterMode::Linear,
                    ..Default::default()
                },
            );
            let render_pass = RenderPass::new(gl.quad_context, texture, None);
            self.output[0] = Some(RenderTarget {
                texture: Texture2D::from_miniquad_texture(texture),
                render_pass,
            });
            copy_fbo(
                internal_id(self.output[1].unwrap()),
                internal_id(self.output[0].unwrap()),
                self.texture_dim,
            );
        }
    }

    pub fn texture_dim(&self) -> (u32, u32) {
        self.texture_dim
    }

    pub fn samples(&self) -> u32 {
        self.samples
    }

    pub fn map_viewport(&self, (x, y, w, h): (i32, i32, i32, i32)) -> (i32, i32, i32, i32) {
        let sx = self.texture_dim.0 as f32 / self.logical_dim.0 as f32;
        let sy = self.texture_dim.1 as f32 / self.logical_dim.1 as f32;
        (
            (x as f32 * sx).round() as i32,
            (y as f32 * sy).round() as i32,
            (w as f32 * sx).round().max(1.) as i32,
            (h as f32 * sy).round().max(1.) as i32,
        )
    }

    pub fn input(&self) -> RenderTarget {
        self.dummy
    }

    pub fn output(&self) -> RenderTarget {
        self.output[0].unwrap()
    }

    pub fn old(&self) -> RenderTarget {
        self.output[1].unwrap()
    }
}

impl Drop for MSRenderTarget {
    fn drop(&mut self) {
        unsafe {
            use miniquad::gl::*;
            glDeleteRenderbuffers(1, &self.rbo as *const _);
            glDeleteFramebuffers(1, &self.fbo as *const _);
        }
        for target in self.output.iter().flatten() {
            target.delete();
        }
    }
}

struct NoteBatchMesh {
    vertices: Vec<Vertex>,
    indices: Vec<u16>,
}

impl NoteBatchMesh {
    fn new() -> Self {
        Self {
            vertices: Vec::with_capacity(MAX_SIZE * 4),
            indices: Vec::with_capacity(MAX_SIZE * 6),
        }
    }

    fn clear(&mut self) {
        self.vertices.clear();
        self.indices.clear();
    }

    fn is_full(&self) -> bool {
        self.vertices.len() + 4 > MAX_SIZE * 4
    }

    fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }

    fn push_quad(&mut self, vertices: [Vertex; 4]) {
        let i = self.vertices.len() as u16;
        self.vertices.extend_from_slice(&vertices);
        self.indices.extend_from_slice(&[i, i + 1, i + 2, i, i + 2, i + 3]);
    }
}

type NoteBatchMap = BTreeMap<(i8, GLuint), Vec<NoteBatchMesh>>;

/// Reusable note quad batcher.
///
/// The previous note path moved the whole map out on every flush, dropping all
/// mesh allocations at frame end. This keeps per-order/texture buffers around
/// and only clears their lengths between frames.
#[derive(Default)]
pub struct NoteBuffer {
    batches: NoteBatchMap,
}

impl NoteBuffer {
    pub fn begin_frame(&mut self) {
        for meshes in self.batches.values_mut() {
            for mesh in meshes {
                mesh.clear();
            }
        }
    }

    pub fn push_quad(&mut self, key: (i8, GLuint), vertices: [Vertex; 4]) {
        let meshes = self.batches.entry(key).or_default();
        if meshes.last().is_none_or(NoteBatchMesh::is_full) {
            meshes.push(NoteBatchMesh::new());
        }
        meshes.last_mut().unwrap().push_quad(vertices);
    }

    pub fn flush(&mut self) {
        let mut gl = unsafe { get_internal_gl() };
        gl.flush();
        let gl = gl.quad_gl;
        gl.draw_mode(DrawMode::Triangles);
        for ((_, tex_id), meshes) in self.batches.iter_mut() {
            gl.texture(Some(Texture2D::from_miniquad_texture(unsafe {
                Texture::from_raw_id(*tex_id, miniquad::TextureFormat::RGBA8)
            })));
            for mesh in meshes.iter() {
                if !mesh.is_empty() {
                    gl.geometry(&mesh.vertices, &mesh.indices);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use macroquad::prelude::WHITE;

    fn vertices() -> [Vertex; 4] {
        std::array::from_fn(|_| Vertex::new(0., 0., 0., 0., 0., WHITE))
    }

    #[test]
    fn note_buffer_groups_by_order_and_texture() {
        let mut buffer = NoteBuffer::default();
        buffer.begin_frame();
        buffer.push_quad((0, 1), vertices());
        buffer.push_quad((0, 1), vertices());
        buffer.push_quad((1, 1), vertices());

        assert_eq!(buffer.batches.len(), 2);
        assert_eq!(buffer.batches[&(0, 1)][0].vertices.len(), 8);
        assert_eq!(buffer.batches[&(1, 1)][0].vertices.len(), 4);
    }

    #[test]
    fn note_buffer_reuses_meshes_after_begin_frame() {
        let mut buffer = NoteBuffer::default();
        buffer.begin_frame();
        buffer.push_quad((0, 1), vertices());
        assert_eq!(buffer.batches[&(0, 1)].len(), 1);
        assert_eq!(buffer.batches[&(0, 1)][0].vertices.len(), 4);

        buffer.begin_frame();
        assert!(buffer.batches[&(0, 1)][0].is_empty());
        buffer.push_quad((0, 1), vertices());
        assert_eq!(buffer.batches[&(0, 1)].len(), 1);
        assert_eq!(buffer.batches[&(0, 1)][0].vertices.len(), 4);
    }

    #[test]
    fn note_buffer_splits_meshes_at_max_batch_size() {
        let mut buffer = NoteBuffer::default();
        buffer.begin_frame();
        for _ in 0..=MAX_SIZE {
            buffer.push_quad((0, 1), vertices());
        }

        assert_eq!(buffer.batches[&(0, 1)].len(), 2);
        assert_eq!(buffer.batches[&(0, 1)][0].vertices.len(), MAX_SIZE * 4);
        assert_eq!(buffer.batches[&(0, 1)][1].vertices.len(), 4);
    }
}
