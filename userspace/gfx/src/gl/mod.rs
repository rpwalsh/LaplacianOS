//! laplacianos-gl — Rust-native OpenGL-equivalent rendering API for LaplacianOS.
//!
//! This is a complete rendering API modelled on OpenGL 3.3 Core Profile,
//! implemented in pure Rust with no C library dependency.  It runs in ring-3
//! userspace and submits work to the kernel GPU executor via `GpuCmd` batches.
//!
//! ## Object model
//!
//! Every GPU object (program, buffer, texture, framebuffer, vertex array) is
//! created through `GlContext` and represented as a typed handle.  The context
//! owns the object registry and the current binding state.
//!
//! ## Shader model
//!
//! Shaders are written in GLSL (or a LaplacianOS IR defined below) and compiled
//! to a bytecode blob stored in `GlProgram`.  Phase 1: the kernel executor
//! uses built-in fixed-function equivalents keyed by the bytecode fingerprint.
//! Phase 2: the bytecode is forwarded to the hardware shader compiler.
//!
//! ## Thread safety
//!
//! `GlContext` is `!Send` and `!Sync` — one context per thread, matching the
//! OpenGL threading model.
//!
//! ## Usage
//!
//! ```no_run
//! use laplacianos_gfx::{GpuDevice, PixelFormat};
//! use laplacianos_gfx::gl::{BufferTarget, BufferUsage, GlContext};
//! use laplacianos_gfx::types::Topology;
//!
//! let device = GpuDevice::open().expect("GPU substrate unavailable");
//! let swapchain = device.create_swapchain(PixelFormat::Bgra8Unorm).unwrap();
//! let mut gl = GlContext::new(device, swapchain.back_buffer());
//! let program = gl.create_program("void main() {}", "void main() {}").unwrap();
//! let vertices = [0_u8; 36];
//! let buffer = gl.gen_buffer();
//! gl.bind_buffer(BufferTarget::ArrayBuffer, buffer);
//! gl.buffer_data_raw(BufferTarget::ArrayBuffer, &vertices, BufferUsage::StaticDraw);
//! gl.use_program(program);
//! gl.draw_arrays(Topology::Triangles, 0, 3);
//! gl.flush();
//! ```

pub mod buffer;
pub mod context;
pub mod draw;
pub mod error;
pub mod framebuffer;
pub mod program;
pub mod state;
pub mod texture;
pub mod uniform;
pub mod vertex;

pub use buffer::{BufferTarget, BufferUsage, GlBuffer};
pub use context::GlContext;
pub use draw::{DrawParams, draw};
pub use error::GlError;
pub use framebuffer::{AttachPoint, FboStatus, GlFbo};
pub use program::{GlProgram, ShaderHint, ShaderStage};
pub use state::{
    BlendEquation, BlendFactor, Capability, CullFace, DepthFunc, FrontFace, PolygonOffset,
    RenderState, ScissorBox, StencilAction, StencilFunc, Viewport,
};
pub use texture::{FilterMode, GlRenderbuffer, GlTexture, SamplerParams, TextureTarget, WrapMode};
pub use uniform::UniformValue;
pub use vertex::{AttribType, GlVao, VertexAttrib, VertexBinding};
